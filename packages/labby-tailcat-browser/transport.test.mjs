import test from 'node:test';
import assert from 'node:assert/strict';
import {TailcatClient} from './transport.mjs';
const cap={origin:'https://depot.example',address:'tcpExample',port:1,peer:{privateKey:'memory-only'},grant:'session-token',generation:'g1',expiresAt:Date.now()+60000,derpMapURL:'https://tailcat.dev/derpmap.json'};
test('expired/missing grants reject before dial',async()=>{
 let calls=0;const dial=async()=>{calls++;throw Error('dialed')};
 await assert.rejects(TailcatClient.connect({...cap,grant:''},{dial}));
 await assert.rejects(TailcatClient.connect({...cap,expiresAt:0},{dial}));assert.equal(calls,0);
});
test('success preserves MCP session and closes streams',async()=>{
 let writes=[],closed=0,id=0;
 const dial=async()=>({write:async bytes=>{writes.push(new TextDecoder().decode(bytes));id=JSON.parse(writes.at(-1).split('\r\n\r\n')[1]).id},read:async()=>{
  const body=JSON.stringify({jsonrpc:'2.0',id,result:{ok:true}});
  return new TextEncoder().encode(`HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: ${body.length}\r\nMcp-Session-Id: session-1\r\n\r\n${body}`)
 },close:()=>closed++});
 const c=await TailcatClient.connect(cap,{dial});await c.request('initialize',{});await c.request('tools/list',{});
 assert.match(writes[1],/Mcp-Session-Id: session-1/);assert.equal(closed,2);c.close();
 await assert.rejects(c.request('tools/list',{}));
});
test('failed mutations never retry and cancellation closes connection',async()=>{
 let writes=0,closed=0;
 const dial=async()=>({write:async()=>{writes++},read:async()=>{throw Error('network failed')},close:()=>closed++});
 const c=await TailcatClient.connect(cap,{dial});await assert.rejects(c.request('tools/call',{name:'sandbox_create'}));assert.equal(writes,1);assert.equal(closed,1);
 const blocked=await TailcatClient.connect(cap,{dial:async()=>({write:async()=>{},read:()=>new Promise(()=>{}),close:()=>closed++})});
 const ac=new AbortController(),pending=blocked.request('tools/list',{}, {signal:ac.signal});setTimeout(()=>ac.abort(),10);await assert.rejects(pending);assert.equal(closed,2);blocked.close();
});
test('eight-operation admission and close cancel pending dials',async()=>{
 let calls=0;const c=await TailcatClient.connect(cap,{dial:()=>{calls++;return new Promise(()=>{})}});
 const pending=Array.from({length:8},()=>c.request('tools/list'));
 await assert.rejects(c.request('tools/list'),/capacity/);assert.equal(calls,8);
 c.close();const results=await Promise.allSettled(pending);assert.ok(results.every(r=>r.status==='rejected'));
});
test('wrong response ID rejects without replay',async()=>{
 let calls=0;const body=JSON.stringify({jsonrpc:'2.0',id:999,result:{}});
 const c=await TailcatClient.connect(cap,{dial:async()=>{calls++;return {write:async()=>{},close:()=>{},read:async()=>new TextEncoder().encode(`HTTP/1.1 200 OK\r\nContent-Length: ${body.length}\r\n\r\n${body}`)}}});
 await assert.rejects(c.request('tools/call',{name:'fixture'}),/response ID/);assert.equal(calls,1);c.close();
});
test('one WASM session owns concurrent streams and closes once',async()=>{
 let sessions=0,dials=0,closes=0;
 const factory=async()=>{sessions++;return {dial:async()=>{dials++;let id;return {write:async bytes=>{id=JSON.parse(new TextDecoder().decode(bytes).split('\r\n\r\n')[1]).id},read:async()=>{const body=JSON.stringify({jsonrpc:'2.0',id,result:{}});return new TextEncoder().encode(`HTTP/1.1 200 OK\r\nContent-Length: ${body.length}\r\n\r\n${body}`)},close:()=>{}}},close:()=>{closes++}}};
 const c=await TailcatClient.connect(cap,{createSession:factory});await Promise.all(Array.from({length:8},()=>c.request('tools/list')));c.close();c.close();assert.equal(sessions,1);assert.equal(dials,8);assert.equal(closes,1);
});
test('pairing origin and generation accompany every MCP request',async()=>{
 let write;
 const body=JSON.stringify({jsonrpc:'2.0',id:1,result:{}});
 const c=await TailcatClient.connect({...cap,origin:'https://depot.example'},{dial:async()=>({
  write:async bytes=>{write=new TextDecoder().decode(bytes)},close:()=>{},
  read:async()=>new TextEncoder().encode(`HTTP/1.1 200 OK\r\nContent-Length: ${body.length}\r\n\r\n${body}`)
 })});
 await c.request('tools/list');c.close();
 assert.match(write,/\r\nOrigin: https:\/\/depot.example\r\n/);
 assert.match(write,/\r\nLabby-Tailcat-Generation: g1\r\n/);
});
test('unsafe or absent origins reject before starting transport',async()=>{
 let calls=0;const dial=async()=>{calls++;throw Error('dialed')};
 for(const origin of [undefined,'http://depot.example','https://depot.example/path','https://user@depot.example','https://depot.example?query','https://depot.example\r\nX: bad']) {
  await assert.rejects(TailcatClient.connect({...cap,origin},{dial}));
 }
 assert.equal(calls,0);
});
test('heartbeat does not interrupt a busy connection and stops on close',async t=>{
 let tick,cleared=false;
 t.mock.method(globalThis,'setInterval',callback=>{tick=callback;return {unref(){}};});
 t.mock.method(globalThis,'clearInterval',()=>{cleared=true;});
 const c=await TailcatClient.connect(cap,{dial:()=>new Promise(()=>{})});
 let pings=0;c.request=async()=>{pings++;};
 c.pending=8;tick();assert.equal(pings,0);assert.equal(c.closed,false);
 c.pending=0;tick();assert.equal(pings,1);
 c.close();assert.equal(cleared,true);
});
