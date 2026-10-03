import {test} from 'node:test';
import assert from 'node:assert/strict';
import {fileURLToPath} from 'node:url';
import {Client} from '@modelcontextprotocol/sdk/client/index.js';
import {StdioClientTransport} from '@modelcontextprotocol/sdk/client/stdio.js';
test('configured adapter publishes atomic cleanup without creating a VM',async()=>{
 const client=new Client({name:'labby-adapter-test',version:'1'});
 const transport=new StdioClientTransport({command:process.execPath,args:[fileURLToPath(new URL('./server.mjs',import.meta.url))],env:{PATH:process.env.PATH},stderr:'pipe'});
 try{
  await client.connect(transport);
  const {tools}=await client.listTools();
  const removal=tools.find(t=>t.name==='sandbox_remove');
  assert.equal(removal._meta['labby.tailcat.atomic_cleanup'],1);
  assert.equal(removal.inputSchema.properties.expectedOwner.type,'string');
  assert.equal(removal.annotations.destructiveHint,true);
  assert.ok(tools.some(t=>t.name==='sandbox_create'));
  assert.ok(tools.some(t=>t.name==='sandbox_exec'));
 }finally{await client.close();await transport.close();}
});
