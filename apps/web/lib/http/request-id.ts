export function withRequestId(message: string, requestId?: string) {
  return requestId ? `${message} (request id: ${requestId})` : message
}
