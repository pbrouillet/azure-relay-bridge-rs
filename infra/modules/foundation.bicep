targetScope = 'resourceGroup'

param name string
param location string
param tags object
param suffix string
@secure()
param githubClientSecret string
@secure()
param cookieKey string

var tableNames = [
  'Workspaces'
  'Members'
  'UserWorkspaces'
  'PlatformAdmins'
  'Connections'
  'Profiles'
  'Gateways'
  'Enrollments'
  'Audit'
  'RevisionMetadata'
]

module network 'br/public:avm/res/network/virtual-network:0.5.2' = {
  name: 'broker-network'
  params: {
    name: 'vnet-${name}'
    location: location
    tags: tags
    addressPrefixes: [
      '10.42.0.0/16'
    ]
    subnets: [
      {
        name: 'container-apps'
        addressPrefix: '10.42.0.0/24'
        delegation: 'Microsoft.App/environments'
      }
      {
        name: 'private-endpoints'
        addressPrefix: '10.42.1.0/27'
        privateEndpointNetworkPolicies: 'Disabled'
      }
    ]
    enableTelemetry: false
  }
}

var privateDnsZoneNames = [
  'privatelink.blob.${environment().suffixes.storage}'
  'privatelink.table.${environment().suffixes.storage}'
  'privatelink${replace(environment().suffixes.keyvaultDns, '.vault.', '.vaultcore.')}'
]

module privateDns 'br/public:avm/res/network/private-dns-zone:0.8.0' = [
  for (zoneName, index) in privateDnsZoneNames: {
    name: 'broker-private-dns-${index}'
    params: {
      name: zoneName
      location: 'global'
      tags: tags
      virtualNetworkLinks: [
        {
          virtualNetworkResourceId: network.outputs.resourceId
          registrationEnabled: false
        }
      ]
      enableTelemetry: false
    }
  }
]

module identity 'br/public:avm/res/managed-identity/user-assigned-identity:0.6.0' = {
  name: 'broker-identity'
  params: {
    name: 'id-${name}'
    location: location
    tags: tags
    enableTelemetry: false
  }
}

module logs 'br/public:avm/res/operational-insights/workspace:0.16.1' = {
  name: 'broker-logs'
  params: {
    name: 'log-${name}'
    location: location
    tags: tags
    dataRetention: 30
    dailyQuotaGb: '1'
    skuName: 'PerGB2018'
    enableTelemetry: false
  }
}

module stack 'br/public:avm/ptn/azd/container-apps-stack:0.4.0' = {
  name: 'broker-container-stack'
  params: {
    location: location
    tags: tags
    // VNet integration is immutable; preserve the original non-VNet environment.
    containerAppsEnvironmentName: 'cae-${name}-vnet'
    infrastructureSubnetResourceId: network.outputs.subnetResourceIds[0]
    internal: false
    workloadProfiles: [
      {
        name: 'Consumption'
        workloadProfileType: 'Consumption'
      }
    ]
    containerRegistryName: 'cr${suffix}'
    logAnalyticsWorkspaceName: logs.outputs.name
    acrSku: 'Basic'
    acrAdminUserEnabled: false
    publicNetworkAccess: 'Enabled'
    zoneRedundant: false
    enableTelemetry: false
  }
}

module storage 'br/public:avm/res/storage/storage-account:0.33.1' = {
  name: 'broker-storage'
  params: {
    name: 'st${suffix}'
    location: location
    tags: tags
    kind: 'StorageV2'
    skuName: 'Standard_LRS'
    allowSharedKeyAccess: false
    defaultToOAuthAuthentication: true
    allowBlobPublicAccess: false
    publicNetworkAccess: 'Disabled'
    privateEndpoints: [
      for (service, index) in ['blob', 'table']: {
        name: 'pe-${name}-${service}'
        service: service
        subnetResourceId: network.outputs.subnetResourceIds[1]
        privateDnsZoneGroup: {
          privateDnsZoneGroupConfigs: [
            {
              privateDnsZoneResourceId: privateDns[index].outputs.resourceId
            }
          ]
        }
        tags: tags
      }
    ]
    supportsHttpsTrafficOnly: true
    minimumTlsVersion: 'TLS1_2'
    requireInfrastructureEncryption: true
    blobServices: {
      isVersioningEnabled: true
      deleteRetentionPolicyEnabled: true
      deleteRetentionPolicyDays: 7
      containerDeleteRetentionPolicyEnabled: true
      containerDeleteRetentionPolicyDays: 7
      containers: [
        {
          name: 'configs'
          publicAccess: 'None'
        }
      ]
    }
    tableServices: {
      tables: [
        for tableName in tableNames: {
          name: tableName
        }
      ]
    }
    roleAssignments: [
      {
        principalId: identity.outputs.principalId
        principalType: 'ServicePrincipal'
        roleDefinitionIdOrName: 'Storage Blob Data Contributor'
      }
      {
        principalId: identity.outputs.principalId
        principalType: 'ServicePrincipal'
        roleDefinitionIdOrName: 'Storage Table Data Contributor'
      }
    ]
    enableTelemetry: false
  }
}

module vault 'br/public:avm/res/key-vault/vault:0.13.3' = {
  name: 'broker-vault'
  params: {
    name: 'kv-${suffix}'
    location: location
    tags: tags
    sku: 'standard'
    enableRbacAuthorization: true
    enableSoftDelete: true
    enablePurgeProtection: true
    enableVaultForDeployment: false
    enableVaultForTemplateDeployment: false
    enableVaultForDiskEncryption: false
    publicNetworkAccess: 'Disabled'
    privateEndpoints: [
      {
        name: 'pe-${name}-vault'
        service: 'vault'
        subnetResourceId: network.outputs.subnetResourceIds[1]
        privateDnsZoneGroup: {
          privateDnsZoneGroupConfigs: [
            {
              privateDnsZoneResourceId: privateDns[2].outputs.resourceId
            }
          ]
        }
        tags: tags
      }
    ]
    roleAssignments: [
      {
        principalId: identity.outputs.principalId
        principalType: 'ServicePrincipal'
        roleDefinitionIdOrName: 'Key Vault Secrets User'
      }
    ]
    diagnosticSettings: [
      {
        workspaceResourceId: logs.outputs.resourceId
      }
    ]
    enableTelemetry: false
  }
}

resource keyVault 'Microsoft.KeyVault/vaults@2025-05-01' existing = {
  name: 'kv-${suffix}'
}

// Keep secret values in secure module parameters, never an ordinary object parameter.
resource githubSecret 'Microsoft.KeyVault/vaults/secrets@2025-05-01' = {
  parent: keyVault
  name: 'github-client-secret'
  tags: tags
  properties: {
    value: githubClientSecret
    contentType: 'GitHub App OAuth client secret'
  }
  dependsOn: [
    vault
  ]
}

resource sessionKey 'Microsoft.KeyVault/vaults/secrets@2025-05-01' = {
  parent: keyVault
  name: 'cookie-key'
  tags: tags
  properties: {
    value: cookieKey
    contentType: 'Base64-encoded 32-byte session encryption key'
  }
  dependsOn: [
    vault
  ]
}

output identityName string = identity.outputs.name
output identityResourceId string = identity.outputs.resourceId
output identityPrincipalId string = identity.outputs.principalId
output identityClientId string = identity.outputs.clientId
output environmentName string = stack.outputs.environmentName
output environmentDefaultDomain string = stack.outputs.defaultDomain
output registryName string = stack.outputs.registryName
output registryLoginServer string = stack.outputs.registryLoginServer
output storageAccountName string = storage.outputs.name
output vaultName string = vault.outputs.name
output vaultUri string = vault.outputs.uri
output logAnalyticsWorkspaceResourceId string = logs.outputs.resourceId
