import { AuthBootstrap } from '@/components/auth/auth-bootstrap'
import { SidebarProvider, SidebarInset } from '@/components/ui/sidebar'
import { AppSidebar } from '@/components/app-sidebar'
import { AppCommandPalette } from '@/components/app-command-palette'
import { Toaster } from '@/components/ui/sonner'

export default function AdminLayout({
  children,
}: {
  children: React.ReactNode
}) {
  return (
    <AuthBootstrap>
      <SidebarProvider>
        <AppSidebar />
        <SidebarInset>{children}</SidebarInset>
        <AppCommandPalette />
        <Toaster />
      </SidebarProvider>
    </AuthBootstrap>
  )
}
