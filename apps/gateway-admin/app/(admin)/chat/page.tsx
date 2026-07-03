import { ChatShell } from '@/components/chat/chat-shell'
import { CapabilityGuard } from '@/components/capability-guard'

export const metadata = {
  title: 'Chat',
  description: 'Admin chat interface',
}

export default function ChatPage() {
  return (
    <CapabilityGuard need="acp" label="Chat">
      <ChatShell />
    </CapabilityGuard>
  )
}
