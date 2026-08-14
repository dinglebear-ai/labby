/**
 * Open a navigable OAuth popup without exposing the admin window to provider
 * content. We explicitly sever `opener` instead of passing the `noopener`
 * window feature because browsers return `null` for a successful noopener
 * open, which is indistinguishable from a popup blocker and prevents callers
 * from navigating the synchronously-created blank tab after an async request.
 */
export function openIsolatedOauthPopup(
  browserWindow: Pick<Window, 'open'> = window,
): Window | null {
  const popup = browserWindow.open('about:blank', '_blank')
  if (!popup) return null

  try {
    popup.opener = null
  } catch {
    popup.close()
    return null
  }
  return popup
}
