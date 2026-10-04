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
  for(const name of ['sandbox_exec','sandbox_inspect','sandbox_list']){
   const tool=tools.find(t=>t.name===name);
   assert.equal(tool._meta['labby.tailcat.owned_access'],1);
   assert.equal(tool.inputSchema.properties.expectedOwner.type,'string');
  }
 const ownedEmpty=await client.callTool({name:'sandbox_list',arguments:{names:[],expectedOwner:'01234567-89ab-4def-8123-456789abcdef'}});
 assert.equal(ownedEmpty.isError,undefined);
 assert.deepEqual(JSON.parse(ownedEmpty.content[0].text).data,[]);
 }finally{await client.close();await transport.close();}
});
