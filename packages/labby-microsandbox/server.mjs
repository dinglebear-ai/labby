#!/usr/bin/env node
import {McpServer} from '@modelcontextprotocol/sdk/server/mcp.js';
import {StdioServerTransport} from '@modelcontextprotocol/sdk/server/stdio.js';
import {Sandbox} from 'microsandbox';
import {z} from 'zod';
import {registerSandboxTools} from 'microsandbox-mcp/dist/tools/sandbox.js';
import {registerExecTools} from 'microsandbox-mcp/dist/tools/exec.js';
import {ok,fail} from 'microsandbox-mcp/dist/utils/response.js';
import {removeOwned} from './cleanup.mjs';
const server=new McpServer({name:'labby-microsandbox',version:'0.0.0'});
const facade={registerTool(name,config,callback){
  if(name!=='sandbox_remove')return server.registerTool(name,config,callback);
  return server.registerTool(name,{
    ...config,
    inputSchema:config.inputSchema.extend({expectedOwner:z.string().uuid().optional()}),
    _meta:{...config._meta,'labby.tailcat.atomic_cleanup':1},
  },async args=>{
    if(args.expectedOwner===undefined)return callback(args);
    try{const result=ok(await removeOwned(args,Sandbox));result._meta={'labby.tailcat.atomic_cleanup':1};return result;}
    catch{return fail('owned_cleanup_failed','Owned cleanup failed; use native recovery for uncertain outcomes.');}
  });
}};
registerSandboxTools(facade);
registerExecTools(server);
await server.connect(new StdioServerTransport());
