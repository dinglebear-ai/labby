import test from 'node:test';
import assert from 'node:assert/strict';
import {HttpDecoder} from './http.mjs';
const bytes=s=>new TextEncoder().encode(s);

test('incremental headers and UTF-8 body lengths',()=>{
 const body=JSON.stringify({id:1,result:'héllo'}),d=new HttpDecoder();
 const wire=bytes(`HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: ${bytes(body).length}\r\nMcp-Session-Id: session\r\n\r\n${body}`);
 for(const b of wire)d.push(new Uint8Array([b]));
 assert.equal(d.done,true);assert.equal(d.headers['mcp-session-id'],'session');
 assert.deepEqual(d.messages,[{id:1,result:'héllo'}]);
});
test('chunked SSE across every byte boundary',()=>{
 const body='event: message\ndata: {"jsonrpc":"2.0","id":2,"result":{}}\n\n';
 const d=new HttpDecoder();
 const wire=bytes(`HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n${bytes(body).length.toString(16)}\r\n${body}\r\n0\r\n\r\n`);
 for(const b of wire)d.push(new Uint8Array([b]));
 assert.equal(d.done,true);assert.equal(d.messages[0].id,2);
});
test('oversize, conflicting length, invalid chunks and incomplete EOF reject',()=>{
 for(const input of [
 'HTTP/1.1 200 OK\r\nContent-Length: 1048577\r\n\r\n',
 'HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n{}',
 'HTTP/1.1 200 OK\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n',
 'HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n'
 ])assert.throws(()=>new HttpDecoder().push(bytes(input)));
 const d=new HttpDecoder();d.push(bytes('HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n{}'));assert.throws(()=>d.finish());
});
test('SSE progress batching is independent of TCP boundaries',()=>{
 const events=Array.from({length:9},(_,i)=>`data: ${JSON.stringify({jsonrpc:'2.0',method:'notifications/progress',params:{progress:i}})}\n\n`).join('');
 const bytes=new TextEncoder().encode(`HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: ${new TextEncoder().encode(events).length}\r\n\r\n${events}`);
 const grouped=new HttpDecoder();grouped.push(bytes);assert.equal(grouped.takeMessages().length,9);
 const split=new HttpDecoder();let count=0;for(const byte of bytes){split.push(Uint8Array.of(byte));count+=split.takeMessages().length}assert.equal(count,9);
});

test('near-limit chunk payload in tiny fragments copies only linear bytes',t=>{
 const body=bytes(JSON.stringify({result:'x'.repeat(1024*1024-64)}));
 const header=bytes(`HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n${body.length.toString(16)}\r\n`);
 let copied=0;const original=Uint8Array.prototype.set;
 t.mock.method(Uint8Array.prototype,'set',function(source,offset){copied+=source.length;return original.call(this,source,offset)});
 const d=new HttpDecoder();d.push(header);for(let i=0;i<body.length;i+=64)d.push(body.subarray(i,i+64));d.push(bytes('\r\n0\r\n\r\n'));
 assert.equal(d.done,true);assert.equal(d.messages[0].result.length,1024*1024-64);
 assert.ok(copied<body.length*4,`copied ${copied} bytes for ${body.length} payload bytes`);
});
test('fragmented chunk terminators remain mandatory',()=>{
 const d=new HttpDecoder();d.push(bytes('HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}'));d.push(bytes('\r'));assert.throws(()=>d.push(bytes('x')));
});
test('large fragmented SSE lines preserve UTF-8 and event budget',()=>{
 const value='é'.repeat(100000),body=bytes(`data: ${JSON.stringify({result:value})}\r\n\r\n`);
 const d=new HttpDecoder();d.push(bytes('HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n'+body.length.toString(16)+'\r\n'));
 for(let i=0;i<body.length;i+=63)d.push(body.subarray(i,i+63));d.push(bytes('\r\n0\r\n\r\n'));assert.deepEqual(d.messages,[{result:value}]);
 const oversized=new HttpDecoder();oversized.push(bytes('HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n'));
 for(let i=0;i<16;i++)oversized.push(bytes('x'.repeat(65536)));assert.throws(()=>oversized.push(bytes('x')));
});
