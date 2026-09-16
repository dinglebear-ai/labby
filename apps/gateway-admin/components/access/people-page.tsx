'use client'

import * as React from 'react'
import { Check, Copy, Loader2, MailPlus, RefreshCw, Users } from 'lucide-react'
import { toast } from 'sonner'

import { AppHeader } from '@/components/app-header'
import { AURORA_MUTED_LABEL, AURORA_PAGE_FRAME } from '@/components/aurora/tokens'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { accessApi, type TeamInvitationCreateOutcome, type TeamSnapshot } from '@/lib/api/access-client'
import { teamInvitationHref } from '@/lib/auth/team-invitation'
import { cn, getErrorMessage } from '@/lib/utils'

const DAY_SECONDS = 24 * 60 * 60

export function PeoplePage() {
  const [teams, setTeams] = React.useState<TeamSnapshot[]>([])
  const [selectedTeamId, setSelectedTeamId] = React.useState('')
  const [email, setEmail] = React.useState('')
  const [role, setRole] = React.useState<TeamInvitationCreateOutcome['role']>('member')
  const [expiresInDays, setExpiresInDays] = React.useState('7')
  const [advanced, setAdvanced] = React.useState(false)
  const [loading, setLoading] = React.useState(true)
  const [submitting, setSubmitting] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)
  const [invitation, setInvitation] = React.useState<{ email: string; link: string; outcome: TeamInvitationCreateOutcome } | null>(null)

  const loadTeams = React.useCallback(async (signal?: AbortSignal) => {
    setLoading(true)
    setError(null)
    try {
      const response = await accessApi.listTeams(signal)
      const active = response.teams.filter((team) => team.status === 'active')
      setTeams(active)
      setSelectedTeamId((current) => active.some((team) => team.team_id === current) ? current : active[0]?.team_id ?? '')
    } catch (cause) {
      if (!signal?.aborted) setError(getErrorMessage(cause, 'Labby could not load your teams.'))
    } finally {
      if (!signal?.aborted) setLoading(false)
    }
  }, [])

  React.useEffect(() => {
    const controller = new AbortController()
    void loadTeams(controller.signal)
    return () => controller.abort()
  }, [loadTeams])

  const selectedTeam = teams.find((team) => team.team_id === selectedTeamId)

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!selectedTeamId || !email.trim()) return
    setSubmitting(true)
    setError(null)
    try {
      const outcome = await accessApi.createTeamInvitation(
        selectedTeamId,
        email,
        role,
        Number(expiresInDays) * DAY_SECONDS,
      )
      const link = teamInvitationHref(window.location.origin, outcome.token)
      setInvitation({ email: email.trim(), link, outcome })
      setEmail('')
      toast.success('Invitation ready to share')
    } catch (cause) {
      setError(getErrorMessage(cause, 'Labby could not create the invitation.'))
    } finally {
      setSubmitting(false)
    }
  }

  async function copyInvitation() {
    if (!invitation) return
    try {
      await navigator.clipboard.writeText(invitation.link)
      toast.success('Invitation link copied')
    } catch {
      toast.error('Copy failed. Select the invitation link manually.')
    }
  }

  return (
    <>
      <AppHeader breadcrumbs={[{ label: 'People' }]} />
      <div className={cn(AURORA_PAGE_FRAME, 'gap-4')}>
        <ConsoleHero
          eyebrow="Access · Teams"
          title="People"
          description="Invite a coworker with their email. Labby handles identity binding, team membership, and the secure one-time handoff."
          pulse={{ color: 'var(--aurora-accent-strong)', label: 'Verified-email invitations' }}
          stats={[
            { label: 'Your teams', value: loading ? '—' : teams.length },
            { label: 'Default role', value: 'Member' },
            { label: 'Default expiry', value: '7 days' },
          ]}
        />

        <div className="grid gap-4 xl:grid-cols-[minmax(0,1.3fr)_minmax(320px,.7fr)]">
          <DashboardPanel title="Invite someone">
            {loading ? (
              <div className="flex min-h-40 items-center justify-center gap-2 text-sm text-aurora-text-muted">
                <Loader2 className="size-4 animate-spin" /> Loading teams…
              </div>
            ) : teams.length === 0 ? (
              <div className="rounded-aurora-2 border border-aurora-border-subtle bg-aurora-control-surface p-5">
                <p className="font-medium text-aurora-text-primary">No active team is available yet.</p>
                <p className="mt-2 text-sm leading-6 text-aurora-text-muted">
                  Finish owner setup first, or create a team with the advanced Access actions. Nothing else in Labby is disabled.
                </p>
                <Button className="mt-4" variant="outline" onClick={() => void loadTeams()}>
                  <RefreshCw className="size-4" /> Refresh
                </Button>
              </div>
            ) : (
              <form className="grid gap-4" onSubmit={submit}>
                <div>
                  <label className="text-sm font-semibold text-aurora-text-primary" htmlFor="invite-email">Email</label>
                  <Input
                    autoComplete="email"
                    className="mt-1.5"
                    id="invite-email"
                    onChange={(event) => setEmail(event.target.value)}
                    placeholder="coworker@example.com"
                    required
                    type="email"
                    value={email}
                  />
                  <p className="mt-1.5 text-xs leading-5 text-aurora-text-muted">
                    They must sign in with this verified email before the invitation can be accepted.
                  </p>
                </div>

                {teams.length > 1 ? (
                  <div>
                    <label className="text-sm font-semibold text-aurora-text-primary" htmlFor="invite-team">Team</label>
                    <Select value={selectedTeamId} onValueChange={setSelectedTeamId}>
                      <SelectTrigger className="mt-1.5" id="invite-team"><SelectValue /></SelectTrigger>
                      <SelectContent>
                        {teams.map((team) => <SelectItem key={team.team_id} value={team.team_id}>{team.name}</SelectItem>)}
                      </SelectContent>
                    </Select>
                  </div>
                ) : (
                  <div className="rounded-aurora-2 border border-aurora-border-subtle bg-aurora-control-surface px-4 py-3">
                    <p className={AURORA_MUTED_LABEL}>Team</p>
                    <p className="mt-1 font-medium text-aurora-text-primary">{selectedTeam?.name}</p>
                  </div>
                )}

                <button
                  className="w-fit text-sm font-medium text-aurora-accent-primary underline-offset-4 hover:underline"
                  onClick={() => setAdvanced((current) => !current)}
                  type="button"
                >
                  {advanced ? 'Use recommended defaults' : 'Role and expiry options'}
                </button>

                {advanced ? (
                  <div className="grid gap-4 rounded-aurora-2 border border-aurora-border-subtle bg-aurora-control-surface p-4 sm:grid-cols-2">
                    <div>
                      <label className="text-sm font-semibold text-aurora-text-primary" htmlFor="invite-role">Role</label>
                      <Select value={role} onValueChange={(value) => setRole(value as TeamInvitationCreateOutcome['role'])}>
                        <SelectTrigger className="mt-1.5" id="invite-role"><SelectValue /></SelectTrigger>
                        <SelectContent>
                          <SelectItem value="member">Member</SelectItem>
                          <SelectItem value="admin">Team admin</SelectItem>
                          <SelectItem value="owner">Owner</SelectItem>
                        </SelectContent>
                      </Select>
                    </div>
                    <div>
                      <label className="text-sm font-semibold text-aurora-text-primary" htmlFor="invite-expiry">Expires</label>
                      <Select value={expiresInDays} onValueChange={setExpiresInDays}>
                        <SelectTrigger className="mt-1.5" id="invite-expiry"><SelectValue /></SelectTrigger>
                        <SelectContent>
                          <SelectItem value="1">In 1 day</SelectItem>
                          <SelectItem value="7">In 7 days</SelectItem>
                          <SelectItem value="30">In 30 days</SelectItem>
                        </SelectContent>
                      </Select>
                    </div>
                  </div>
                ) : (
                  <p className="text-xs text-aurora-text-muted">Recommended defaults: Member · expires in 7 days.</p>
                )}

                {error ? <p className="rounded-aurora-2 border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive">{error}</p> : null}
                <Button className="w-full sm:w-fit" disabled={submitting || !selectedTeamId || !email.trim()} size="lg" type="submit">
                  {submitting ? <Loader2 className="size-4 animate-spin" /> : <MailPlus className="size-4" />}
                  {submitting ? 'Creating invitation…' : 'Create invitation'}
                </Button>
              </form>
            )}
          </DashboardPanel>

          <DashboardPanel title="Share the invitation">
            {invitation ? (
              <div className="grid gap-4">
                <div className="flex items-start gap-3 rounded-aurora-2 border border-aurora-success/30 bg-aurora-success/10 p-4">
                  <Check className="mt-0.5 size-5 shrink-0 text-aurora-success" />
                  <div className="min-w-0">
                    <p className="font-medium text-aurora-text-primary">Invitation ready</p>
                    <p className="mt-1 break-all text-sm text-aurora-text-muted">{invitation.email}</p>
                  </div>
                </div>
                <div>
                  <p className={AURORA_MUTED_LABEL}>Secure invite link</p>
                  <Input className="mt-1.5 font-mono text-xs" readOnly value={invitation.link} />
                  <p className="mt-2 text-xs leading-5 text-aurora-text-muted">
                    The invitation secret stays after the # fragment, so browsers do not send it to proxies, access logs, or referrer headers.
                  </p>
                </div>
                <Button onClick={() => void copyInvitation()}>
                  <Copy className="size-4" /> Copy invitation link
                </Button>
                <Button variant="outline" onClick={() => setInvitation(null)}>Invite another person</Button>
              </div>
            ) : (
              <div className="flex min-h-52 flex-col items-center justify-center rounded-aurora-2 border border-dashed border-aurora-border-subtle px-6 text-center">
                <Users className="size-8 text-aurora-text-subtle" />
                <p className="mt-3 font-medium text-aurora-text-primary">One link, then sign in</p>
                <p className="mt-2 max-w-sm text-sm leading-6 text-aurora-text-muted">
                  The recipient opens the link, signs in with the invited email, and Labby joins them to the team. No principal IDs or manually generated tokens.
                </p>
              </div>
            )}
          </DashboardPanel>
        </div>
      </div>
    </>
  )
}
