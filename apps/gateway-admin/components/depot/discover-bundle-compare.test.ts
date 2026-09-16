import test from 'node:test'
import assert from 'node:assert/strict'
import { bundleCompareRows, canCompareBundles } from './discover-bundle-compare'
import type { FederatedArtifact } from '@/lib/api/depot-client'

const artifact = (name:string): FederatedArtifact => ({ providerId:'ard', artifactId:name, title:name })

test('bundle compare is available only for the reference bundles with known contents', () => {
  assert.equal(canCompareBundles([artifact('project-a-loadout'), artifact('oncall-loadout')]), true)
  assert.equal(canCompareBundles([artifact('project-a-loadout')]), false)
  assert.equal(canCompareBundles([artifact('project-a-loadout'), artifact('repo-triage')]), false)
})

test('bundle comparison preserves shared and one-sided members by kind', () => {
  const rows=bundleCompareRows(artifact('project-a-loadout'), artifact('oncall-loadout'))
  assert.deepEqual(rows.find(row=>row.name==='rust-reviewer'), { kind:'Agent', name:'rust-reviewer', inA:true, inB:true })
  assert.deepEqual(rows.find(row=>row.name==='playwright'), { kind:'MCP', name:'playwright', inA:true, inB:false })
  assert.deepEqual(rows.find(row=>row.name==='cost-ceiling'), { kind:'Hook', name:'cost-ceiling', inA:false, inB:true })
})
