/** Keep host setup header adaptation out of unrelated route bundles. */
export function nativeOperatorHeaders(action: string, serviceLabel: string) {
  if (serviceLabel !== 'Setup' || !['tailcat.configure', 'tailcat.enroll', 'tailcat.enable'].includes(action)) {
    throw new TypeError('Native operator context is reserved for Tailcat setup actions')
  }
  return (original: HeadersInit | undefined): Headers => {
    const headers = new Headers(original)
    headers.delete('x-labby-team-id')
    headers.delete('x-labby-project-id')
    return headers
  }
}
