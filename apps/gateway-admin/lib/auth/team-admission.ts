import { loadBrowserSession } from './session-store.ts'

/**
 * Durable team admission (the server's domain Viewer policy) runs as `/v1`
 * middleware, but an unprovisioned session only ever calls `/auth/session`.
 * Without one `/v1` request the policy can never admit a qualifying identity,
 * so the no-access screen calls this once: a read-only, CSRF-free request that
 * lets the server admit the caller if its own policy allows, then reloads the
 * session. The browser never decides admission; a non-qualifying identity
 * simply stays unprovisioned.
 */
export async function requestTeamAdmission(): Promise<void> {
  try {
    await fetch('/v1/catalog', { method: 'GET', credentials: 'include', cache: 'no-store' })
  } catch {
    // Admission is best-effort; the session reload below reports the result.
  }
  await loadBrowserSession()
}
