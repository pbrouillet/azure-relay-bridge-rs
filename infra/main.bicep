targetScope = 'subscription'

@description('Azure Developer CLI environment name. Used as a tag, not an unrestricted resource name.')
@minLength(1)
@maxLength(64)
param environmentName string

@description('Azure region for broker resources. Relay workspaces may use any allowedLocations region.')
param location string

@description('GitHub App client ID, not the numeric App ID.')
@minLength(1)
param githubClientId string

@secure()
@minLength(1)
@description('GitHub App OAuth client secret. Never included in deployment outputs.')
param githubClientSecret string

@secure()
@minLength(44)
@maxLength(44)
@description('Standard base64 encoding of exactly 32 cryptographically random bytes.')
param cookieKey string

@minLength(1)
@description('Comma-separated, nonzero numeric GitHub user IDs allowed to bootstrap platform administration.')
param bootstrapAdminIds string

@minLength(1)
@description('Comma-separated Azure region codes in which workspace Relay namespaces may be created.')
param allowedLocations string

@description('Set automatically by azd. Preserve an existing broker image during subsequent provisioning.')
param brokerExists bool = false

var suffix = uniqueString(subscription().subscriptionId, environmentName, location)
var name = 'azb-${suffix}'
var tags = {
  'azd-env-name': environmentName
  application: 'azbridge-broker'
}
var brokerName = 'ca-${name}'

resource appResourceGroup 'Microsoft.Resources/resourceGroups@2025-04-01' = {
  name: 'rg-${name}'
  location: location
  tags: tags
}

resource relayResourceGroup 'Microsoft.Resources/resourceGroups@2025-04-01' = {
  name: 'rg-${name}-relay'
  location: location
  tags: tags
}

module foundation './modules/foundation.bicep' = {
  name: 'broker-foundation'
  scope: appResourceGroup
  params: {
    name: name
    location: location
    tags: tags
    suffix: suffix
    githubClientSecret: githubClientSecret
    cookieKey: cookieKey
  }
}

module relayPermissions './modules/relay-permissions.bicep' = {
  name: 'broker-relay-permissions'
  scope: relayResourceGroup
  params: {
    principalId: foundation.outputs.identityPrincipalId
  }
}

module broker './modules/broker.bicep' = {
  name: 'broker-app'
  scope: appResourceGroup
  params: {
    name: brokerName
    location: location
    tags: tags
    environmentName: foundation.outputs.environmentName
    environmentDefaultDomain: foundation.outputs.environmentDefaultDomain
    registryName: foundation.outputs.registryName
    identityName: foundation.outputs.identityName
    identityResourceId: foundation.outputs.identityResourceId
    identityPrincipalId: foundation.outputs.identityPrincipalId
    identityClientId: foundation.outputs.identityClientId
    vaultUri: foundation.outputs.vaultUri
    storageAccountName: foundation.outputs.storageAccountName
    relayResourceGroupName: relayResourceGroup.name
    githubClientId: githubClientId
    bootstrapAdminIds: bootstrapAdminIds
    allowedLocations: allowedLocations
    exists: brokerExists
  }
  dependsOn: [
    relayPermissions
  ]
}

output AZURE_RESOURCE_GROUP string = appResourceGroup.name
output AZURE_RELAY_RESOURCE_GROUP string = relayResourceGroup.name
output AZURE_LOCATION string = location
output AZURE_CONTAINER_APP_NAME string = brokerName
output AZURE_CONTAINER_REGISTRY_ENDPOINT string = foundation.outputs.registryLoginServer
output AZURE_CONTAINER_REGISTRY_NAME string = foundation.outputs.registryName
output AZURE_CONTAINER_ENVIRONMENT_NAME string = foundation.outputs.environmentName
output AZURE_CONTAINER_ENVIRONMENT_DEFAULT_DOMAIN string = foundation.outputs.environmentDefaultDomain
output AZURE_STORAGE_ACCOUNT_NAME string = foundation.outputs.storageAccountName
output AZURE_KEY_VAULT_NAME string = foundation.outputs.vaultName
output AZURE_KEY_VAULT_URI string = foundation.outputs.vaultUri
output AZURE_LOG_ANALYTICS_WORKSPACE_ID string = foundation.outputs.logAnalyticsWorkspaceResourceId
output BROKER_IDENTITY_NAME string = foundation.outputs.identityName
output BROKER_IDENTITY_RESOURCE_ID string = foundation.outputs.identityResourceId
output BROKER_IDENTITY_PRINCIPAL_ID string = foundation.outputs.identityPrincipalId
output BROKER_IDENTITY_CLIENT_ID string = foundation.outputs.identityClientId
output BROKER_PUBLIC_URL string = broker.outputs.publicUrl
output GITHUB_CALLBACK_URL string = '${broker.outputs.publicUrl}/auth/github/callback'
