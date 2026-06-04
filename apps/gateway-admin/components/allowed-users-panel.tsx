'use client'

import { useState, useEffect, useCallback } from 'react'
import { Loader2, Plus, Trash2, Users } from 'lucide-react'
import { toast } from 'sonner'
import { AURORA_STRONG_PANEL } from '@/components/aurora/tokens'
import { cn, getErrorMessage } from '@/lib/utils'
import {
  authAdminApi,
  AuthAdminApiError,
  type AllowedEmailEntry,
} from '@/lib/api/auth-admin-client'

export function AllowedUsersPanel() {
  const [entries, setEntries] = useState<AllowedEmailEntry[]>([])
  const [isLoading, setIsLoading] = useState(true)
  const [loadError, setLoadError] = useState<string | null>(null)

  const [addEmail, setAddEmail] = useState('')
  const [isAdding, setIsAdding] = useState(false)
  const [addError, setAddError] = useState<string | null>(null)

  const [removingEmail, setRemovingEmail] = useState<string | null>(null)

  const loadEntries = useCallback(async () => {
    setIsLoading(true)
    setLoadError(null)
    try {
      const data = await authAdminApi.listAllowedEmails()
      setEntries(data)
    } catch (err) {
      setLoadError(getErrorMessage(err, 'Failed to load allowed users'))
    } finally {
      setIsLoading(false)
    }
  }, [])

  useEffect(() => {
    void loadEntries()
  }, [loadEntries])

  async function handleAdd(event: React.FormEvent) {
    event.preventDefault()
    const email = addEmail.trim()
    if (!email) return

    setIsAdding(true)
    setAddError(null)
    try {
      await authAdminApi.addAllowedEmail(email)
      setAddEmail('')
      toast.success(`${email} added to the allowlist.`)
      await loadEntries()
    } catch (err) {
      if (err instanceof AuthAdminApiError && err.status === 422) {
        setAddError(err.message || 'This email is already in the allowlist.')
      } else {
        setAddError(getErrorMessage(err, 'Failed to add email'))
      }
    } finally {
      setIsAdding(false)
    }
  }

  async function handleRemove(entry: AllowedEmailEntry) {
    setRemovingEmail(entry.email)
    try {
      await authAdminApi.removeAllowedEmail(entry.email)
      toast.success(`${entry.email} removed from the allowlist.`)
      await loadEntries()
    } catch (err) {
      toast.error(getErrorMessage(err, 'Failed to remove email'))
    } finally {
      setRemovingEmail(null)
    }
  }

  return (
    <div className={cn(AURORA_STRONG_PANEL, 'px-6 py-5')}>
      <div className="flex items-center gap-3">
        <Users className="size-5 text-aurora-accent-primary" />
        <div>
          <p className="text-base font-semibold text-aurora-text-primary">Allowed users</p>
          <p className="mt-0.5 text-sm text-aurora-text-muted">
            Only these email addresses can sign in via OAuth.
          </p>
        </div>
      </div>

      {/* Add form */}
      <form onSubmit={(e) => void handleAdd(e)} className="mt-5 flex gap-2">
        <div className="flex-1">
          <label htmlFor="allowed-email-input" className="sr-only">
            Email address to allow
          </label>
          <input
            id="allowed-email-input"
            type="email"
            value={addEmail}
            onChange={(e) => {
              setAddEmail(e.target.value)
              setAddError(null)
            }}
            placeholder="user@example.com"
            disabled={isAdding}
            className={cn(
              'w-full rounded-aurora-1 border bg-aurora-control-surface px-3 py-2 text-sm text-aurora-text-primary placeholder:text-aurora-text-muted',
              'focus:outline-none focus:ring-2 focus:ring-aurora-accent-primary/50',
              addError
                ? 'border-aurora-error'
                : 'border-aurora-border-strong',
            )}
          />
          {addError ? (
            <p className="mt-1.5 text-xs text-aurora-error" role="alert">
              {addError}
            </p>
          ) : null}
        </div>
        <button
          type="submit"
          disabled={isAdding || !addEmail.trim()}
          aria-label="Add email to allowlist"
          className={cn(
            'inline-flex items-center gap-1.5 rounded-aurora-1 border border-aurora-accent-primary/60 bg-aurora-accent-primary/10 px-3 py-2 text-sm font-medium text-aurora-accent-primary',
            'hover:bg-aurora-accent-primary/20 disabled:cursor-not-allowed disabled:opacity-50',
            'focus:outline-none focus:ring-2 focus:ring-aurora-accent-primary/50',
          )}
        >
          {isAdding ? (
            <Loader2 className="size-4 animate-spin" />
          ) : (
            <Plus className="size-4" />
          )}
          Add
        </button>
      </form>

      {/* Table */}
      <div className="mt-4">
        {isLoading ? (
          <div className="space-y-2">
            {Array.from({ length: 3 }, (_, index) => (
              <div
                key={index}
                className="h-10 animate-pulse rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface"
              />
            ))}
          </div>
        ) : loadError ? (
          <div className="rounded-aurora-1 border border-aurora-error/30 bg-aurora-error/8 p-4 text-sm text-aurora-error">
            {loadError}
          </div>
        ) : entries.length === 0 ? (
          <div className="rounded-aurora-1 border border-aurora-border-strong border-dashed p-6 text-center text-sm text-aurora-text-muted">
            No users in the allowlist yet.
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm" aria-label="Allowed users">
              <thead>
                <tr className="border-b border-aurora-border-strong text-left text-xs text-aurora-text-muted">
                  <th scope="col" className="pb-2 pr-4 font-medium">Email</th>
                  <th scope="col" className="pb-2 pr-4 font-medium">Added by</th>
                  <th scope="col" className="pb-2 pr-4 font-medium">Added</th>
                  <th scope="col" className="pb-2 font-medium">
                    <span className="sr-only">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {entries.map((entry) => (
                  <tr
                    key={entry.email}
                    className="border-b border-aurora-border-strong/50 last:border-0"
                  >
                    <td className="py-2.5 pr-4 font-medium text-aurora-text-primary">
                      {entry.email}
                    </td>
                    <td className="py-2.5 pr-4 text-aurora-text-muted">
                      {entry.added_by}
                    </td>
                    <td className="py-2.5 pr-4 text-aurora-text-muted">
                      {new Date(entry.created_at).toLocaleDateString()}
                    </td>
                    <td className="py-2.5">
                      <button
                        type="button"
                        aria-label={`Remove ${entry.email}`}
                        onClick={() => void handleRemove(entry)}
                        disabled={removingEmail === entry.email}
                        className={cn(
                          'inline-flex items-center gap-1 rounded px-2 py-1 text-xs text-aurora-error',
                          'hover:bg-aurora-error/10 focus:outline-none focus:ring-2 focus:ring-aurora-error/40 disabled:cursor-not-allowed disabled:opacity-50',
                        )}
                      >
                        {removingEmail === entry.email ? (
                          <Loader2 className="size-3.5 animate-spin" />
                        ) : (
                          <Trash2 className="size-3.5" />
                        )}
                        Remove
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

    </div>
  )
}
