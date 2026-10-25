import { AuthBootstrap } from '@/components/auth/auth-bootstrap'
import { ConsoleShell } from '@/components/console/console-shell'
import { AppCommandPaletteEntry } from '@/components/app-command-palette-entry'
import { Toaster } from '@/components/ui/sonner'
import { CapabilityRouteBoundary } from '@/components/auth/capability-route-boundary'

export default function AdminLayout({
  children,
}: {
  children: React.ReactNode
}) {
  return (
    <AuthBootstrap>
      <ConsoleShell>
        <CapabilityRouteBoundary>{children}</CapabilityRouteBoundary>
      </ConsoleShell>
      <AppCommandPaletteEntry />
      <Toaster />
    </AuthBootstrap>
  )
}
