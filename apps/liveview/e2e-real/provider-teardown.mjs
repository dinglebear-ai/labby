import {cleanupOwnedProviderOutput} from './provider-run-output.mjs'
import {cleanupOwnedProviderTls} from './provider-run-ownership.mjs'

export default async function(config) {
  const owner = config.metadata?.providerOutputOwner
  const outputClean = !owner || cleanupOwnedProviderOutput(owner)
  cleanupOwnedProviderTls(config.metadata?.providerTlsOwner)
  if (!outputClean) throw Error('PROVIDER_RUN_OUTPUT_CLEANUP_UNCONFIRMED')
}
