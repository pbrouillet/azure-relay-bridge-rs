"""Offline assertions over compiled templates; no Azure account or third-party packages."""

import json
import ipaddress
from pathlib import Path
import shutil
import subprocess
import sys
import unittest


def resources(template):
    values = template.get("resources", [])
    return list(values.values()) if isinstance(values, dict) else values


def deployment(template, name):
    return next(
        resource
        for resource in resources(template)
        if resource.get("type") == "Microsoft.Resources/deployments"
        and resource.get("name") == name
    )


class InfrastructureContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if len(sys.argv) == 3:
            cls.main, cls.revision = [
                json.loads(Path(path).read_text(encoding="utf-8-sig"))
                for path in sys.argv[1:]
            ]
        else:
            root = Path(__file__).resolve().parents[1]
            templates = []
            for name in ("main", "broker"):
                result = subprocess.run(
                    [shutil.which("az") or "az", "bicep", "build",
                     "--file", str(root / f"{name}.bicep"), "--stdout"],
                    capture_output=True,
                    encoding="utf-8",
                    check=False,
                )
                if result.returncode:
                    raise RuntimeError(result.stderr)
                templates.append(json.loads(result.stdout))
            cls.main, cls.revision = templates
        cls.foundation = deployment(cls.main, "broker-foundation")["properties"]["template"]
        cls.broker = deployment(cls.main, "broker-app")["properties"]["template"]
        cls.app = deployment(cls.broker, "broker-container-app")["properties"]["parameters"]

    def test_subscription_scope_and_separate_resource_groups(self):
        self.assertIn("subscriptionDeploymentTemplate", self.main["$schema"])
        groups = [
            resource for resource in resources(self.main)
            if resource.get("type") == "Microsoft.Resources/resourceGroups"
        ]
        self.assertEqual(len(groups), 2)
        self.assertNotEqual(groups[0]["name"], groups[1]["name"])
        self.assertIn("azd-env-name", self.main["variables"]["tags"])
        for group in groups:
            self.assertEqual(group["tags"], "[variables('tags')]")

    def test_secrets_remain_secure_across_module_boundary(self):
        for template in (self.main, self.foundation):
            for name in ("githubClientSecret", "cookieKey"):
                self.assertEqual(template["parameters"][name]["type"].lower(), "securestring")
            for output in template.get("outputs", {}).values():
                value = json.dumps(output)
                self.assertNotIn("parameters('githubClientSecret')", value)
                self.assertNotIn("parameters('cookieKey')", value)
        secrets = [
            resource for resource in resources(self.foundation)
            if resource.get("type") == "Microsoft.KeyVault/vaults/secrets"
        ]
        self.assertEqual(len(secrets), 2)

    def test_storage_tables_private_blobs_and_oauth(self):
        parameters = deployment(self.foundation, "broker-storage")["properties"]["parameters"]
        for name, expected in (
            ("allowSharedKeyAccess", False),
            ("allowBlobPublicAccess", False),
            ("defaultToOAuthAuthentication", True),
            ("supportsHttpsTrafficOnly", True),
        ):
            self.assertEqual(parameters[name]["value"], expected)
        self.assertEqual(set(self.foundation["variables"]["tableNames"]), {
            "Workspaces", "Members", "UserWorkspaces", "PlatformAdmins",
            "Connections", "Profiles", "Gateways", "Enrollments", "Audit",
            "RevisionMetadata",
        })
        blobs = parameters["blobServices"]["value"]
        self.assertEqual(blobs["containers"], [{"name": "configs", "publicAccess": "None"}])
        self.assertTrue(blobs["isVersioningEnabled"])
        roles = parameters["roleAssignments"]["value"]
        self.assertEqual({role["roleDefinitionIdOrName"] for role in roles}, {
            "Storage Blob Data Contributor", "Storage Table Data Contributor",
        })

    def test_key_vault_uses_rbac_and_only_secret_read_role(self):
        parameters = deployment(self.foundation, "broker-vault")["properties"]["parameters"]
        self.assertTrue(parameters["enableRbacAuthorization"]["value"])
        self.assertTrue(parameters["enablePurgeProtection"]["value"])
        self.assertEqual(
            parameters["roleAssignments"]["value"][0]["roleDefinitionIdOrName"],
            "Key Vault Secrets User",
        )
        self.assertNotIn("secrets", parameters)

    def test_private_network_has_separate_delegated_and_endpoint_subnets(self):
        parameters = deployment(self.foundation, "broker-network")["properties"]["parameters"]
        address_space = ipaddress.ip_network(parameters["addressPrefixes"]["value"][0])
        apps, endpoints = parameters["subnets"]["value"]
        app_network = ipaddress.ip_network(apps["addressPrefix"])
        endpoint_network = ipaddress.ip_network(endpoints["addressPrefix"])
        self.assertTrue(app_network.subnet_of(address_space))
        self.assertTrue(endpoint_network.subnet_of(address_space))
        self.assertFalse(app_network.overlaps(endpoint_network))
        self.assertLessEqual(app_network.prefixlen, 27)
        self.assertEqual(apps["delegation"], "Microsoft.App/environments")
        self.assertNotIn("delegation", endpoints)
        self.assertEqual(endpoints["privateEndpointNetworkPolicies"], "Disabled")

    def test_new_environment_uses_vnet_and_keeps_public_ingress(self):
        parameters = deployment(
            self.foundation, "broker-container-stack"
        )["properties"]["parameters"]
        self.assertIn("-vnet", parameters["containerAppsEnvironmentName"]["value"])
        self.assertIn(
            "subnetResourceIds.value[0]", parameters["infrastructureSubnetResourceId"]["value"]
        )
        self.assertFalse(parameters["internal"]["value"])
        self.assertEqual(parameters["publicNetworkAccess"]["value"], "Enabled")
        self.assertEqual(parameters["acrSku"]["value"], "Basic")
        self.assertEqual(parameters["workloadProfiles"]["value"], [
            {"name": "Consumption", "workloadProfileType": "Consumption"},
        ])

    def test_private_dependencies_have_endpoints_and_linked_dns(self):
        zones = self.foundation["variables"]["privateDnsZoneNames"]
        self.assertEqual(len(zones), 3)
        self.assertIn("privatelink.blob.", zones[0])
        self.assertIn("privatelink.table.", zones[1])
        self.assertIn("replace(environment().suffixes.keyvaultDns, '.vault.', '.vaultcore.')", zones[2])
        dns = next(
            resource for resource in resources(self.foundation)
            if "broker-private-dns-" in resource.get("name", "")
        )
        links = dns["properties"]["parameters"]["virtualNetworkLinks"]["value"]
        self.assertEqual(len(links), 1)
        self.assertFalse(links[0]["registrationEnabled"])
        self.assertIn("outputs.resourceId", links[0]["virtualNetworkResourceId"])
        storage = deployment(self.foundation, "broker-storage")["properties"]["parameters"]
        vault = deployment(self.foundation, "broker-vault")["properties"]["parameters"]
        for parameters in (storage, vault):
            self.assertEqual(parameters["publicNetworkAccess"]["value"], "Disabled")
            endpoints = json.dumps(parameters["privateEndpoints"])
            self.assertIn("subnetResourceIds.value[1]", endpoints)
            self.assertIn("privateDnsZoneResourceId", endpoints)
        storage_endpoints = storage["privateEndpoints"]["copy"][0]
        self.assertEqual(storage_endpoints["count"], "[length(createArray('blob', 'table'))]")
        vault_endpoint = vault["privateEndpoints"]["value"][0]
        self.assertEqual(vault_endpoint["service"], "vault")
        self.assertIn(
            "2", vault_endpoint["privateDnsZoneGroup"]["privateDnsZoneGroupConfigs"][0]
            ["privateDnsZoneResourceId"],
        )

    def test_vault_uri_waits_for_the_vault_module(self):
        value = self.foundation["outputs"]["vaultUri"]["value"]
        compiled_resources = self.foundation["resources"]
        if isinstance(compiled_resources, dict):
            symbol = next(
                name for name, resource in compiled_resources.items()
                if resource == deployment(self.foundation, "broker-vault")
            )
            self.assertEqual(value, f"[reference('{symbol}').outputs.uri.value]")
        else:
            symbol = "broker-vault"
            self.assertIn("Microsoft.Resources/deployments', 'broker-vault'", value)
            self.assertIn(".outputs.uri.value", value)
        self.assertNotIn("Microsoft.KeyVault/vaults", value)
        for resource in resources(self.foundation):
            if resource.get("type") == "Microsoft.KeyVault/vaults/secrets":
                self.assertTrue(
                    any(symbol in dependency for dependency in resource["dependsOn"])
                )

    def test_relay_owner_is_scoped_to_dedicated_group(self):
        module = deployment(self.main, "broker-relay-permissions")
        self.assertIn("-relay", module["resourceGroup"])
        assignments = resources(module["properties"]["template"])
        self.assertEqual(len(assignments), 1)
        self.assertIn(
            "2787bf04-f1f5-4bfe-8383-c8a24483ee38",
            assignments[0]["properties"]["roleDefinitionId"],
        )
        self.assertEqual(assignments[0]["properties"]["principalType"], "ServicePrincipal")

    def test_https_identity_and_single_replica(self):
        self.assertTrue(self.app["external"]["value"])
        self.assertFalse(self.app["ingressAllowInsecure"]["value"])
        self.assertEqual(self.app["containerMinReplicas"]["value"], 1)
        self.assertEqual(self.app["containerMaxReplicas"]["value"], 1)
        self.assertEqual(self.app["identityType"]["value"], "UserAssigned")
        self.assertIn("registryName", self.app["containerRegistryName"]["value"])
        self.assertIn("'azd-service-name', 'broker'", self.app["tags"]["value"])

    def test_production_probes_and_placeholder_are_distinct(self):
        probes = self.app["containerProbes"]["value"]
        self.assertEqual({probe["type"] for probe in probes}, {"Liveness", "Readiness"})
        for probe in probes:
            self.assertIn("'/'", probe["httpGet"]["path"])
            self.assertIn("'/healthz'", probe["httpGet"]["path"])
            self.assertIn("80, 8080", probe["httpGet"]["port"])
        self.assertIn("reference(resourceId('Microsoft.App/containerApps'", self.app["imageName"])
        for parameter in ("env", "secrets"):
            self.assertIn("variables('placeholder')", self.app[parameter])
            self.assertIn("createObject('value', createArray())", self.app[parameter])

    def test_revision_uses_built_image_and_all_runtime_variables(self):
        revision_broker = deployment(self.revision, "broker-revision")
        self.assertEqual(
            revision_broker["properties"]["parameters"]["imageName"]["value"],
            "[parameters('imageName')]",
        )
        expected = {
            "PUBLIC_URL", "LISTEN_ADDR", "GITHUB_CLIENT_ID", "GITHUB_CLIENT_SECRET",
            "COOKIE_KEY", "BOOTSTRAP_ADMIN_IDS", "STORAGE_ACCOUNT",
            "AZURE_SUBSCRIPTION_ID", "RELAY_RESOURCE_GROUP", "ALLOWED_LOCATIONS",
            "AZURE_CLIENT_ID", "STATIC_DIR",
        }
        # Conditional ARM expressions retain the original environment names as literals.
        env = self.app["env"]
        for name in expected:
            self.assertIn(f"'{name}'", env)
        self.assertIn("'secretRef', 'github-client-secret'", env)
        self.assertIn("'secretRef', 'cookie-key'", env)
        self.assertIn("https://", self.broker["variables"]["publicUrl"])
        self.assertNotIn("container-app", self.broker["variables"]["publicUrl"])

    def test_azd_parameter_files_match_templates(self):
        root = Path(__file__).resolve().parents[1]
        for name, template in (("main", self.main), ("broker", self.revision)):
            parameters = json.loads((root / f"{name}.parameters.json").read_text())
            self.assertEqual(set(parameters["parameters"]), set(template["parameters"]))
            for parameter in parameters["parameters"].values():
                self.assertRegex(parameter["value"], r"^\$\{[A-Z_]+\}$")


if __name__ == "__main__":
    if len(sys.argv) not in (1, 3):
        raise SystemExit("usage: python infra\\tests\\test_contract.py [MAIN_JSON REVISION_JSON]")
    unittest.main(argv=[sys.argv[0]], verbosity=2)
