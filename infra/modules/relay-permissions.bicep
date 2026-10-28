targetScope = 'resourceGroup'

param principalId string

resource relayOwner 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(resourceGroup().id, principalId, 'azbridge-relay-owner')
  properties: {
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', '2787bf04-f1f5-4bfe-8383-c8a24483ee38')
    principalId: principalId
    principalType: 'ServicePrincipal'
  }
}
