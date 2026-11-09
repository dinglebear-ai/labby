import test from 'node:test'
import assert from 'node:assert/strict'
import {normalizePreferences, readPreferences, writePreferences, downloadSnapshot, focusTarget} from './observe_preferences.mjs'
const key = 'a'.repeat(64)
test('preferences never persist records queries cursors or identities', () => {
 assert.deepEqual(normalizePreferences({tab:'logs', window:'7d', query:'private', rows:[1], selected:'1', cursor:'private'}), {tab:'logs',window:'7d'})
 assert.deepEqual(normalizePreferences(null), {})
 assert.deepEqual(normalizePreferences({tab:'evil', window:'30d'}), {})
})
test('invalid namespaces and oversized malformed storage fail closed', () => {
 assert.deepEqual(readPreferences({getItem(){throw Error()}}, key), {})
 assert.deepEqual(readPreferences({getItem(){return '{'}}, key), {})
 assert.deepEqual(readPreferences({getItem(){return ' '.repeat(1025)}}, key), {})
 assert.equal(writePreferences({setItem(){throw Error()}}, key, {}), false)
 assert.equal(writePreferences({setItem(){throw Error('must not touch')}}, 'private', {}), false)
})
test('storage writes only bounded allowlisted preferences', () => {
 let value
 const storage = {setItem(k,v){value=[k,v]}}
 assert.equal(writePreferences(storage,key,{tab:'logs',window:'1h',rows:['secret']}),true)
 assert.deepEqual(JSON.parse(value[1]),{tab:'logs',window:'1h'})
 assert.match(value[0], /^labby:observe:[a-f0-9]{64}:v1$/)
})
test('download refuses oversized or mismatched identity payloads', () => {
 assert.equal(downloadSnapshot({key:'b'.repeat(64),data:'{}'},key,{}),false)
 assert.equal(downloadSnapshot({key,data:'x'.repeat(65537)},key,{}),false)
})
test('drawer focus returns to existing trigger or safe main after refresh removal', () => {
 const trigger = {}, main = {}
 const document = {getElementById(id){return id === 'observe-inspect-1' ? trigger : null}}
 assert.equal(focusTarget(document,'1',main),trigger)
 assert.equal(focusTarget(document,'removed',main),main)
})
test('successful download uses fixed safe filename type and revokes the object URL', () => {
 const events = []
 const anchor = {click(){events.push('click')}, remove(){events.push('remove')}}
 const env = {
   Blob: class {constructor(parts, options){events.push(options.type)}},
   URL: {createObjectURL(){events.push('create'); return 'blob:owned'}, revokeObjectURL(url){events.push(`revoke:${url}`)}},
   document: {createElement(){return anchor}, body:{appendChild(){events.push('append')}}}
 }
 assert.equal(downloadSnapshot({key,data:'{}'},key,env),true)
 assert.equal(anchor.download,'labby-observe-loaded-snapshot.json')
 assert.deepEqual(events,['application/json;charset=utf-8','create','append','click','remove','revoke:blob:owned'])
})
