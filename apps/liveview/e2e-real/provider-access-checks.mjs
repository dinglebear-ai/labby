// Shared only by the native acceptance test and its offline regressions.
export function teamInventoryRow(page, team) {
  return page.locator('#access-card-teams article').filter({has: page.getByText(team.name, {exact: true})})
}

export async function withInvitationClipboard(page, token, receipt, exercise) {
  let primary
  receipt.clipboardCleanup = 'unconfirmed'
  try {
    await exercise()
  } catch (error) {
    primary = error
  } finally {
    let timer
    try {
      receipt.clipboardCleanup = await Promise.race([
        page.evaluate(async value => {
          const deadline = Date.now() + 1500
          const current = await navigator.clipboard.readText()
          if (Date.now() > deadline) return 'unconfirmed'
          if (current !== value) return 'owned-content-absent'
          await navigator.clipboard.writeText('')
          return await navigator.clipboard.readText() === '' ? 'cleared' : 'unconfirmed'
        }, token),
        new Promise(resolve => { timer = setTimeout(() => resolve('unconfirmed'), 2000) }),
      ])
    } catch {
      receipt.clipboardCleanup = 'unconfirmed'
    } finally {
      clearTimeout(timer)
    }
  }
  if (primary) throw primary
  if (receipt.clipboardCleanup === 'unconfirmed') throw Error('ACCESS_CLIPBOARD_CLEANUP_UNCONFIRMED')
}
