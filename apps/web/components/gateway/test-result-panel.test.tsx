import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import type { Gateway } from '@/lib/types/gateway'
installTestDom()
Object.defineProperty(globalThis, 'NodeFilter', { value: window.NodeFilter, configurable: true })
Object.defineProperty(globalThis, 'HTMLInputElement', { value: window.HTMLInputElement, configurable: true })

test('probe panel renders known empty and unavailable families distinctly', async () => {
 const { TestResultPanel } = await import('./test-result-panel')
 const unknown = {state:'unknown' as const, discovered:null, exposed:null}
 const result = {success:true,message:'Connected',discovered_tools:0,discovered_resources:0,discovered_prompts:0,capability_observation:{scope:'credential' as const,tools:{state:'known' as const,discovered:0,exposed:0},resources:unknown,prompts:{state:'failed' as const,discovered:null,exposed:null},skills:unknown}}
 const view = await renderClient(<TestResultPanel result={{gateway:{name:'linear'} as Gateway,result}} onClose={()=>{}} />)
 const markup = document.body.innerHTML
 assert.match(markup,/0\/0/)
 assert.match(markup,/Not discovered/)
 assert.match(markup,/Discovery failed/)
 await view.unmount()
})
