targetScope = 'resourceGroup'

param environmentName string
param location string
param name string
param containerAppsEnvironmentName string
param environmentDefaultDomain string
param registryName string
param identityName string
param identityResourceId string
param identityPrincipalId string
param identityClientId string
param vaultUri string
param storageAccountName string
param relayResourceGroupName string
param githubClientId string
param bootstrapAdminIds string
param allowedLocations string
param imageName string

// azd's revision-based deployment applies the image and its 8080 /healthz configuration together.
module broker './modules/broker.bicep' = {
  name: 'broker-revision'
  params: {
    name: name
    location: location
    tags: {
      'azd-env-name': environmentName
      application: 'azbridge-broker'
    }
    environmentName: containerAppsEnvironmentName
    environmentDefaultDomain: environmentDefaultDomain
    registryName: registryName
    identityName: identityName
    identityResourceId: identityResourceId
    identityPrincipalId: identityPrincipalId
    identityClientId: identityClientId
    vaultUri: vaultUri
    storageAccountName: storageAccountName
    relayResourceGroupName: relayResourceGroupName
    githubClientId: githubClientId
    bootstrapAdminIds: bootstrapAdminIds
    allowedLocations: allowedLocations
    imageName: imageName
    exists: true
  }
}

output BROKER_PUBLIC_URL string = broker.outputs.publicUrl
