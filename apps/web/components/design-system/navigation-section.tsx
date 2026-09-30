import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from '@/components/ui/breadcrumb'
import { Button } from '@/components/ui/button'
import {
  Pagination,
  PaginationContent,
  PaginationItem,
  PaginationLink,
  PaginationNext,
  PaginationPrevious,
} from '@/components/ui/pagination'
import {
  Sidebar,
  SidebarContent,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarProvider,
} from '@/components/ui/sidebar'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { cn } from '@/lib/utils'
import {
  AURORA_ACCENT_CTA,
  AURORA_DISPLAY_2,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
  AURORA_STRONG_PANEL,
  controlTone,
} from '@/components/aurora/tokens'
import { navigationTabs, sidebarPreviewItems } from './demo-data'

export function NavigationSection() {
  return (
    <section className={cn(AURORA_STRONG_PANEL, 'overflow-hidden')}>
      <div className="border-b border-aurora-border-strong px-5 py-4">
        <p className={AURORA_MUTED_LABEL}>Navigation</p>
        <h2 className={cn(AURORA_DISPLAY_2, 'mt-2 text-aurora-text-primary')}>
          Routing and wayfinding patterns
        </h2>
        <p className="mt-2 max-w-2xl text-sm text-aurora-text-muted">
          Compare breadcrumbs, tabs, toolbar actions, pagination, and sidebar item emphasis without
          changing the real app navigation.
        </p>
      </div>

      <div className="grid gap-4 px-5 py-5 xl:grid-cols-[minmax(0,1.1fr)_minmax(0,0.9fr)]">
        <div className="space-y-4">
          <div className={cn(AURORA_MEDIUM_PANEL, 'space-y-4 px-4 py-4')}>
            <p className="text-sm font-medium text-aurora-text-primary">Breadcrumbs</p>
            <Breadcrumb>
              <BreadcrumbList>
                <BreadcrumbItem>
                  <BreadcrumbLink href="/">Overview</BreadcrumbLink>
                </BreadcrumbItem>
                <BreadcrumbSeparator />
                <BreadcrumbItem>
                  <BreadcrumbLink href="/gateways">Gateways</BreadcrumbLink>
                </BreadcrumbItem>
                <BreadcrumbSeparator />
                <BreadcrumbItem>
                  <BreadcrumbPage>Design review</BreadcrumbPage>
                </BreadcrumbItem>
              </BreadcrumbList>
            </Breadcrumb>
          </div>

          <div className={cn(AURORA_MEDIUM_PANEL, 'space-y-4 px-4 py-4')}>
            <div className="flex flex-wrap items-center justify-between gap-3">
              <p className="text-sm font-medium text-aurora-text-primary">Tabs</p>
              <div className="flex flex-wrap gap-2">
                <Button variant="outline" className={cn(controlTone(), 'rounded-aurora-1 hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}>
                  Filter logs
                </Button>
                <Button className={AURORA_ACCENT_CTA}>
                  Create gateway
                </Button>
              </div>
            </div>
            <Tabs defaultValue={navigationTabs[1]}>
              <TabsList className="bg-aurora-control-surface text-aurora-text-muted">
                {navigationTabs.map((tab) => (
                  <TabsTrigger key={tab} value={tab}>
                    {tab}
                  </TabsTrigger>
                ))}
              </TabsList>
              {navigationTabs.map((tab) => (
                <TabsContent key={tab} value={tab} className="rounded-[1rem] border border-aurora-border-strong bg-aurora-control-surface px-4 py-4 text-sm text-aurora-text-muted">
                  {tab} keeps related operators within the same page shell.
                </TabsContent>
              ))}
            </Tabs>
          </div>
        </div>

        <div className="space-y-4">
          <div className={cn(AURORA_MEDIUM_PANEL, 'space-y-4 px-4 py-4')}>
            <p className="text-sm font-medium text-aurora-text-primary">Sidebar item states</p>
            <div className="overflow-hidden rounded-[1rem] border border-aurora-border-strong bg-aurora-nav-bg">
              <SidebarProvider defaultOpen>
                <div className="flex min-h-[220px] bg-aurora-nav-bg">
                  <Sidebar collapsible="none" className="w-full border-r-0">
                    <SidebarContent>
                      <SidebarGroup>
                        <SidebarGroupLabel>Navigation</SidebarGroupLabel>
                        <SidebarGroupContent>
                          <SidebarMenu>
                            {sidebarPreviewItems.map((item) => (
                              <SidebarMenuItem key={item.label}>
                                <SidebarMenuButton isActive={item.active}>
                                  <span>{item.label}</span>
                                </SidebarMenuButton>
                              </SidebarMenuItem>
                            ))}
                          </SidebarMenu>
                        </SidebarGroupContent>
                      </SidebarGroup>
                    </SidebarContent>
                  </Sidebar>
                </div>
              </SidebarProvider>
            </div>
          </div>

          <div className={cn(AURORA_MEDIUM_PANEL, 'space-y-4 px-4 py-4')}>
            <p className="text-sm font-medium text-aurora-text-primary">Pagination</p>
            <Pagination className="justify-start">
              <PaginationContent>
                <PaginationItem>
                  <PaginationPrevious href="#prev" />
                </PaginationItem>
                <PaginationItem>
                  <PaginationLink href="#page-1">1</PaginationLink>
                </PaginationItem>
                <PaginationItem>
                  <PaginationLink href="#page-2" isActive>
                    2
                  </PaginationLink>
                </PaginationItem>
                <PaginationItem>
                  <PaginationLink href="#page-3">3</PaginationLink>
                </PaginationItem>
                <PaginationItem>
                  <PaginationNext href="#next" />
                </PaginationItem>
              </PaginationContent>
            </Pagination>
          </div>
        </div>
      </div>
    </section>
  )
}
