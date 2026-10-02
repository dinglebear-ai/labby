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
