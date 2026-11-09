import {cleanupOwnedProviderTls} from './provider-run-ownership.mjs'

export default async function(config) {
  cleanupOwnedProviderTls(config.metadata?.providerTlsOwner)
}
