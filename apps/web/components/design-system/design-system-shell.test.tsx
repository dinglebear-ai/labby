import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { DesignSystemShell } from './design-system-shell'
import { SidebarProvider } from '@/components/ui/sidebar'

test('design system shell renders all primary section headings', () => {
  const markup = renderToStaticMarkup(
    React.createElement(
      SidebarProvider,
      null,
      React.createElement(DesignSystemShell),
    ),
  )

  assert.match(markup, /Theme Foundations/)
  assert.match(markup, /Controls/)
  assert.match(markup, /Feedback/)
  assert.match(markup, /Navigation/)
  assert.match(markup, /Command Palette/)
  assert.match(markup, /Data Display/)
  assert.match(markup, /Application Patterns/)
  assert.match(markup, /Aurora page background/)
  assert.match(markup, /Display 1/)
  assert.match(markup, /Radius 3/)
  assert.match(markup, /Tier 2 lift/)
})
