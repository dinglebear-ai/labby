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
export interface TeamAdmissionAttempt {
  admissionError?: string
}

export async function requestTeamAdmission(): Promise<TeamAdmissionAttempt> {
  let admissionError: string | undefined
  try {
    const response = await fetch('/v1/catalog', { method: 'GET', credentials: 'include', cache: 'no-store' })
    if (!response.ok) {
      admissionError = `Team admission probe returned HTTP ${response.status}.`
    }
  } catch (error) {
    admissionError = `Team admission probe could not reach Labby: ${error instanceof Error ? error.message : 'network request failed'}.`
  }
  // Always reload. The admission middleware can update authority even when the
  // probe response itself is not useful to the browser.
  await loadBrowserSession()
  return { admissionError }
}
