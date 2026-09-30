'use client'

import { CheckCircle2, XCircle, Clock, Wrench, FileText, MessageSquare, X } from 'lucide-react'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Button } from '@/components/ui/button'
import type { Gateway, TestGatewayResult } from '@/lib/types/gateway'
import {
  DETAIL_NO_DATA,
  DETAIL_STAT_GRID_STYLE,
  DetailMicroLabel,
  DetailStatCard,
} from './gateway-detail-chrome'

interface TestResultPanelProps {
  result: { gateway: Gateway; result: TestGatewayResult } | null
  onClose: () => void
}

export function TestResultPanel({ result, onClose }: TestResultPanelProps) {
  if (!result) return null

  const { gateway, result: testResult } = result
  const severity = testResult.severity ?? (testResult.success ? 'success' : 'failure')
  const isSuccess = severity === 'success'
  const isWarning = severity === 'warning'

  return (
    <Sheet open={!!result} onOpenChange={(open) => !open && onClose()}>
      <SheetContent className="sm:max-w-md">
        <SheetHeader>
          <SheetTitle>Connection Test Results</SheetTitle>
          <SheetDescription>
            Test results for {gateway.name}
          </SheetDescription>
        </SheetHeader>

        {/* px-4 matches SheetHeader's p-4 — without it the body ran flush to
            the sheet edges and the stat grid clipped on the right. */}
        <div className="mt-2 space-y-6 px-4">
          {/* Status */}
          <div
            style={{ borderRadius: 9 }}
            className={`flex items-start gap-4 border p-4 ${
            isSuccess
              ? 'border-aurora-success/20 bg-aurora-success/5'
              : isWarning
                ? 'border-aurora-warn/20 bg-aurora-warn/5'
                : 'border-aurora-error/20 bg-aurora-error/5'
          }`}>
            {isSuccess ? (
              <CheckCircle2 className="size-5 text-aurora-success mt-0.5" />
            ) : isWarning ? (
              <Clock className="size-5 text-aurora-warn mt-0.5" />
            ) : (
              <XCircle className="size-5 text-aurora-error mt-0.5" />
            )}
            <div className="flex-1">
              <p className={`font-medium ${
                isSuccess
                  ? 'text-aurora-success'
                  : isWarning
                    ? 'text-aurora-warn'
                    : 'text-aurora-error'
              }`}>
                {isSuccess
                  ? 'Connection Successful'
                  : isWarning
                    ? 'Connection Successful with Warnings'
                    : 'Connection Failed'}
              </p>
              <p className="text-sm text-aurora-text-muted mt-0.5">
                {testResult.message}
              </p>
              {testResult.detail && (
                <p className="text-sm text-aurora-warn mt-2 font-mono bg-aurora-warn/10 rounded px-2 py-1">
                  {testResult.detail}
                </p>
              )}
              {testResult.error && (
                <p className="text-sm text-aurora-error mt-2 font-mono bg-aurora-error/10 rounded px-2 py-1">
                  {testResult.error}
                </p>
              )}
              {(isWarning || !testResult.success) && (
                <p className="mt-2 text-xs text-aurora-text-muted">
                  {isWarning
                    ? 'The server connected, but at least one optional MCP primitive could not be discovered. The note above is the exact operator-facing guidance returned by the server backend.'
                    : 'Check the server transport, auth source, and any required stdio environment variables. The probe message above is the exact last failure returned by the server backend.'}
                </p>
              )}
            </div>
          </div>

          {/* Metrics */}
          {(testResult.success ||
            testResult.discovered_tools !== undefined ||
            testResult.discovered_resources !== undefined ||
            testResult.discovered_prompts !== undefined) && (
            <div className="space-y-3">
              <DetailMicroLabel>
                {isSuccess ? 'Connection Details' : 'Probe Details'}
              </DetailMicroLabel>

              {/* Mock stat-card chrome — see gateway-detail-chrome.tsx. */}
              <div style={DETAIL_STAT_GRID_STYLE}>
                {testResult.latency_ms !== undefined && (
                  <DetailStatCard
                    icon={<Clock size={11} />}
                    label="Latency"
                    value={`${testResult.latency_ms}ms`}
                  />
                )}

                {testResult.discovered_tools !== undefined && (
                  <DetailStatCard
                    icon={<Wrench size={11} />}
                    label="Tools"
                    value={testResult.discovered_tools ?? DETAIL_NO_DATA}
                    sub="discovered"
                  />
                )}

                {testResult.discovered_resources !== undefined && (
                  <DetailStatCard
                    icon={<FileText size={11} />}
                    label="Resources"
                    value={testResult.discovered_resources ?? DETAIL_NO_DATA}
                    sub="discovered"
                  />
                )}

                {testResult.discovered_prompts !== undefined && (
                  <DetailStatCard
                    icon={<MessageSquare size={11} />}
                    label="Prompts"
                    value={testResult.discovered_prompts ?? DETAIL_NO_DATA}
                    sub="discovered"
                  />
                )}
              </div>
            </div>
          )}
        </div>

        <div className="mt-8 px-4 pb-4">
          <Button variant="outline" onClick={onClose} className="w-full">
            <X className="size-4 mr-2" />
            Close
          </Button>
        </div>
      </SheetContent>
    </Sheet>
  )
}
