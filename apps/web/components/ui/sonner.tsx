'use client'

import { useTheme } from 'next-themes'
import { Toaster as Sonner, ToasterProps } from 'sonner'

const Toaster = ({ ...props }: ToasterProps) => {
  const { theme = 'system' } = useTheme()

  return (
    <Sonner
      theme={theme as ToasterProps['theme']}
      className="toaster group"
      style={
        {
          '--normal-bg': 'var(--aurora-panel-strong)',
          '--normal-text': 'var(--aurora-text-primary)',
          '--normal-border': 'var(--aurora-border-default)',
          '--success-bg': 'var(--aurora-success)',
          '--success-text': 'var(--aurora-page-bg)',
          '--success-border': 'var(--aurora-success)',
          '--error-bg': 'var(--aurora-error)',
          '--error-text': 'var(--aurora-page-bg)',
          '--error-border': 'var(--aurora-error)',
        } as React.CSSProperties
      }
      {...props}
    />
  )
}

export { Toaster }
