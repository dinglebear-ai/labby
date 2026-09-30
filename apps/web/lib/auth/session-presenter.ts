type SessionUser = {
  sub: string
  email?: string | null
}

export function sessionPrimaryEmail(user: SessionUser) {
  return user.email?.trim() || user.sub
}

export function sessionAvatarFallback(user: SessionUser) {
  const label = sessionPrimaryEmail(user).trim()
  const initial = label.charAt(0)
  return initial ? initial.toUpperCase() : '?'
}
