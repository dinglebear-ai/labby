import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { Download, Search } from 'lucide-react'

import { Button } from './button'
import { DropdownMenu, DropdownMenuTrigger } from './dropdown-menu'

test('icon-led Button derives an accessible name and tooltip from its text', () => {
  const markup = renderToStaticMarkup(<Button><Search />Discover</Button>)

  assert.match(markup, /aria-label="Discover"/)
  assert.match(markup, /title="Discover"/)
  assert.match(markup, /data-slot="button"/)
})

test('Button preserves explicit labels and supports asChild links', () => {
  const explicit = renderToStaticMarkup(
    <Button aria-label="Search the catalog" title="Catalog search"><Search />Discover</Button>,
  )
  assert.match(explicit, /aria-label="Search the catalog"/)
  assert.match(explicit, /title="Catalog search"/)

  const link = renderToStaticMarkup(<Button asChild><a href="/depot"><Search />Discover</a></Button>)
  assert.match(link, /href="\/depot"/)
  assert.match(link, /aria-label="Discover"/)
  assert.match(link, /title="Discover"/)
})

test('DropdownMenuTrigger derives its accessible name and tooltip', () => {
  const markup = renderToStaticMarkup(
    <DropdownMenu><DropdownMenuTrigger><Download />Export</DropdownMenuTrigger></DropdownMenu>,
  )

  assert.match(markup, /data-slot="dropdown-menu-trigger"/)
  assert.match(markup, /aria-label="Export"/)
  assert.match(markup, /title="Export"/)
})

test('text-only Button retains its visible text contract', () => {
  const markup = renderToStaticMarkup(<Button>Save changes</Button>)

  assert.match(markup, />Save changes<\/button>/)
  assert.doesNotMatch(markup, /<svg/)
})
