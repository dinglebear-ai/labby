import {test} from 'node:test'
import assert from 'node:assert/strict'
import {readLayout,writeLayout} from '../../assets/js/overview_layout.mjs'
const first='a'.repeat(64),second='b'.repeat(64)
function store(){const rows=new Map();return {getItem:key=>rows.get(key),setItem:(key,value)=>rows.set(key,value),rows}}
test('layout persistence is isolated and only known UI card values survive',()=>{
 const s=store()
 assert.equal(writeLayout(s,first,{order:['calls','secret','traffic','calls'],widths:{traffic:true,inventory:true},credential:'private'}),true)
 assert.deepEqual(readLayout(s,first),{order:['calls','traffic'],widths:{traffic:true}})
 assert.deepEqual(readLayout(s,second),{})
 assert.equal([...s.rows.values()].some(v=>v.includes('private')),false)
 assert.equal(writeLayout(s,'unverified',{order:[]}),false)
})
test('malformed oversized or unavailable storage is safe',()=>{
 for(const value of ['{','[]','x'.repeat(4097),'null']){
  assert.deepEqual(readLayout({getItem:()=>value},first),{})
 }
 assert.deepEqual(readLayout({getItem(){throw Error('blocked')}},first),{})
 assert.equal(writeLayout({setItem(){throw Error('blocked')}},first,{}),false)
})
