import test from 'node:test';
import assert from 'node:assert/strict';
import {loadTailcat,bindDeliveryIdentity} from './wasm.mjs';
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
test('delivery opening uses its own identity once, including failed attempts',async()=>{
 const calls=[];
 const identity=bindDeliveryIdentity({privateKey:'in-memory-key',publicKey:'browser'},(...args)=>{calls.push(args);return {grant:'private'};});
 const sender='nodekey:'+'a'.repeat(64);
 assert.deepEqual(await identity.openDelivery(sender,'sealed'),{grant:'private'});
 assert.deepEqual(calls,[['in-memory-key',sender,'sealed']]);
 await assert.rejects(identity.openDelivery(sender,'sealed'),/Invalid delivery/);
 const failed=bindDeliveryIdentity({privateKey:'key'},()=>{throw Error('Invalid delivery');});
 await assert.rejects(failed.openDelivery(sender,'sealed'));
 await assert.rejects(failed.openDelivery(sender,'sealed'));
});
test('delivery wrapper rejects malformed and oversized packets before decryption',async()=>{
 let calls=0;
 const identity=bindDeliveryIdentity({privateKey:'key'},()=>{calls++;});
 await assert.rejects(identity.openDelivery('bad','cipher'));
 await assert.rejects(identity.openDelivery('nodekey:'+'a'.repeat(64),'x'.repeat(21901)));
 assert.equal(calls,0);
});
