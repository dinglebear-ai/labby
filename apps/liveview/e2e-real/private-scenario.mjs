export async function runPrivateScenario({page, info, accounting, cleanup}, run) {
  page.setDefaultTimeout(10000)
  page.setDefaultNavigationTimeout(15000)
  let stage = 'initialization'
  let failure
  let cleanupStatus = 'complete'
  const step = async (label, operation) => {
    stage = label
    return operation()
  }
  try {
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
  try {
    await info.attach('private-config-action-accounting', {
      body: JSON.stringify({...accounting, cleanup: cleanupStatus}, null, 2),
      contentType: 'application/json',
    })
  } catch {
    failure ||= new Error('PRIVATE_SCENARIO_RECEIPT_FAILED')
  }
  if (failure) throw failure
}
