import {rmSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'

export default async function() {
  const path = process.env.LABBY_E2E_PROVIDER_TLS
  if (process.env.LABBY_E2E_PROVIDER_TLS_OWNED === 'true' &&
      path?.startsWith(join(tmpdir(), 'labby-controlled-idp-'))) {
    rmSync(path, {recursive: true, force: true})
  }
}
