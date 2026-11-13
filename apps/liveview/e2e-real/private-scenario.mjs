import {writeFileSync} from 'node:fs'
import {readConsoleHttpCounts, consoleHttpDelta} from './console-http-counts.mjs'

export async function runPrivateScenario({page, info, accounting, cleanup, consoleHttpReceipt}, run) {
  page.setDefaultTimeout(10000)
  page.setDefaultNavigationTimeout(15000)
  let stage = 'initialization'
  let failure
  let cleanupStatus = 'complete'
  let baseline
  let actual = {}
  const step = async (label, operation) => {
    stage = label
    return operation()
  }
  try {
    if (consoleHttpReceipt) baseline = readConsoleHttpCounts(consoleHttpReceipt)
    await run(step)
  } catch (error) {
    const kind = error?.name === 'TimeoutError' ? 'timeout' : 'action'
    failure = new Error(`PRIVATE_SCENARIO_FAILED stage=${stage} kind=${kind}`)
  }
  try {
    if (page.isClosed()) {
      cleanupStatus = 'page-closed'
    } else {
      await cleanup()
    }
  } catch {
    cleanupStatus = 'unconfirmed'
  }
  if (cleanupStatus !== 'complete' && !failure) {
    failure = new Error('PRIVATE_SCENARIO_CLEANUP_FAILED')
  }
  if (consoleHttpReceipt) {
    try {
      actual = consoleHttpDelta(baseline, readConsoleHttpCounts(consoleHttpReceipt), accounting.gatewayDirect)
      // These scenario limits are final postconditions. The BEAM guard enforces
      // the separate whole-run 2048-request ceiling before Finch transport.
      if (actual.gatewayActualCombined > 40) failure ||= new Error('PRIVATE_SCENARIO_GATEWAY_ACTUAL_BUDGET_EXCEEDED')
      if (actual.consoleHttpActual > 128) failure ||= new Error('PRIVATE_SCENARIO_CONSOLE_HTTP_BUDGET_EXCEEDED')
    } catch {
      failure ||= new Error('PRIVATE_SCENARIO_ACCOUNTING_FAILED')
    }
  }
  try {
    const body = JSON.stringify({...accounting, ...actual, cleanup: cleanupStatus}, null, 2)
    if (Buffer.byteLength(body) > 16384) throw Error()
    if (info.outputPath) {
      const path = info.outputPath('private-config-action-accounting.json')
      writeFileSync(path, body, {mode: 0o600})
      await info.attach('private-config-action-accounting', {path, contentType: 'application/json'})
    } else {
      await info.attach('private-config-action-accounting', {body, contentType: 'application/json'})
    }
  } catch {
    failure ||= new Error('PRIVATE_SCENARIO_RECEIPT_FAILED')
  }
  if (failure) throw failure
}
