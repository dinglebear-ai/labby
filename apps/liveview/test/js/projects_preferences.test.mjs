import test from 'node:test'
import assert from 'node:assert/strict'
import {readPreferences,writePreferences} from '../../assets/js/projects_preferences.mjs'
const key='a'.repeat(64)
const storage=()=>{const rows=new Map();return {getItem:k=>rows.get(k)??null,setItem:(k,v)=>rows.set(k,v)}}
test('preferences persist only bounded presentation fields in caller and gateway namespace',()=>{
 const store=storage()
 writePreferences(store,key,{layout:'grid',query:'private search',team:'t',current:'dABw',role:'admin',cookie:'secret'})
 assert.deepEqual(readPreferences(store,key),{layout:'grid',query:'private search',team:'t',current:'dABw'})
 assert.deepEqual(readPreferences(store,'b'.repeat(64)),{})
})
test('invalid namespace and oversized or malformed browser values fail closed',()=>{
 const store=storage()
 assert.equal(writePreferences(store,'attacker',{}),false)
 store.setItem(`labby:projects:${key}:v1`,'{')
 assert.deepEqual(readPreferences(store,key),{})
 store.setItem(`labby:projects:${key}:v1`,'x'.repeat(4097))
 assert.deepEqual(readPreferences(store,key),{})
 writePreferences(store,key,{layout:'html',query:'x'.repeat(201),team:'x'.repeat(129),current:'x'.repeat(345)})
 assert.deepEqual(readPreferences(store,key),{})
})
