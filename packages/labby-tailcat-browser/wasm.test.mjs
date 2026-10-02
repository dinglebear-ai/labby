import test from 'node:test';
import assert from 'node:assert/strict';
import {loadTailcat} from './wasm.mjs';
const hash='a'.repeat(64);
test('loader rejects query-bearing asset roots before fetching',async()=>{
 globalThis.location=new URL('https://depot.test/ui/sandboxes');
 await assert.rejects(loadTailcat({baseURL:'/assets/tailcat/?token=secret',wasmSha256:hash,runtimeSha256:hash}),/asset root/);
});
test('loader cancels oversized streamed artifacts before allocation',async()=>{
 globalThis.location=new URL('https://depot.test/ui/sandboxes');
 let cancelled=false;
 globalThis.fetch=async()=>({ok:true,headers:new Headers(),body:new ReadableStream({pull(c){c.enqueue(new Uint8Array(1024*1024))},cancel(){cancelled=true}})});
 await assert.rejects(loadTailcat({baseURL:'/assets/tailcat/',wasmSha256:hash,runtimeSha256:hash}),/budget/);
 assert.equal(cancelled,true);
});
