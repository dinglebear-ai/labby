import type { Metadata } from 'next'
import { ConsoleShell } from '@/components/console/console-shell'
import { PublicSetupPage } from '@/components/setup/public-setup-page'

export const metadata: Metadata = {
  title: 'Setup | Labby',
  description: 'Set up Labby with Codex, public HTTPS, Google OAuth, and ChatGPT.',
}

export default function SetupPage() {
  return (
    <ConsoleShell publicSetup>
      <PublicSetupPage />
    </ConsoleShell>
  )
}
