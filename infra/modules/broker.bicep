targetScope = 'resourceGroup'

param name string
param location string
param tags object
param environmentName string
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
param imageName string = ''
param exists bool = false

var placeholder = 'mcr.microsoft.com/azuredocs/containerapps-helloworld:latest'
resource existingApp 'Microsoft.App/containerApps@2025-01-01' existing = if (exists) {
  name: name
}

// Provision before packaging is supported. Re-provisioning must not revert a deployed broker.
var effectiveImage = !empty(imageName) ? imageName : exists ? existingApp!.properties.template.containers[0].image : placeholder
var isPlaceholder = effectiveImage == placeholder
var targetPort = isPlaceholder ? 80 : 8080
var healthPath = isPlaceholder ? '/' : '/healthz'
var publicUrl = 'https://${name}.${environmentDefaultDomain}'

module app 'br/public:avm/ptn/azd/acr-container-app:0.5.0' = {
  name: 'broker-container-app'
  params: {
    name: name
    location: location
    tags: union(tags, {
      'azd-service-name': 'broker'
    })
    containerAppsEnvironmentName: environmentName
    containerRegistryName: registryName
    identityName: identityName
    identityType: 'UserAssigned'
    userAssignedIdentityResourceId: identityResourceId
    principalId: identityPrincipalId
    imageName: effectiveImage
    containerName: 'broker'
    external: true
    ingressAllowInsecure: false
    ingressTransport: 'http'
    targetPort: targetPort
    revisionMode: 'Single'
    containerMinReplicas: 1
    containerMaxReplicas: 1
    containerCpuCoreCount: '0.5'
    containerMemory: '1Gi'
    containerProbes: [
      {
        type: 'Liveness'
        httpGet: {
          path: healthPath
          port: targetPort
          scheme: 'HTTP'
        }
        initialDelaySeconds: 15
        periodSeconds: 30
        timeoutSeconds: 3
        failureThreshold: 3
      }
      {
        type: 'Readiness'
        httpGet: {
          path: healthPath
          port: targetPort
          scheme: 'HTTP'
        }
        initialDelaySeconds: 5
        periodSeconds: 10
        timeoutSeconds: 3
        failureThreshold: 3
      }
    ]
    secrets: isPlaceholder ? [] : [
      {
        name: 'github-client-secret'
        keyVaultUrl: '${vaultUri}secrets/github-client-secret'
        identity: identityResourceId
      }
      {
        name: 'cookie-key'
        keyVaultUrl: '${vaultUri}secrets/cookie-key'
        identity: identityResourceId
      }
    ]
    env: isPlaceholder ? [] : [
      { name: 'PUBLIC_URL', value: publicUrl }
      { name: 'LISTEN_ADDR', value: '0.0.0.0:8080' }
      { name: 'GITHUB_CLIENT_ID', value: githubClientId }
      { name: 'GITHUB_CLIENT_SECRET', secretRef: 'github-client-secret' }
      { name: 'COOKIE_KEY', secretRef: 'cookie-key' }
      { name: 'BOOTSTRAP_ADMIN_IDS', value: bootstrapAdminIds }
      { name: 'STORAGE_ACCOUNT', value: storageAccountName }
      { name: 'AZURE_SUBSCRIPTION_ID', value: subscription().subscriptionId }
      { name: 'RELAY_RESOURCE_GROUP', value: relayResourceGroupName }
      { name: 'ALLOWED_LOCATIONS', value: allowedLocations }
      { name: 'AZURE_CLIENT_ID', value: identityClientId }
      { name: 'STATIC_DIR', value: '/app/web/dist' }
    ]
    enableTelemetry: false
  }
}

output publicUrl string = publicUrl
