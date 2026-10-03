#!/usr/bin/env node
import {McpServer} from '@modelcontextprotocol/sdk/server/mcp.js';
import {StdioServerTransport} from '@modelcontextprotocol/sdk/server/stdio.js';
import {Sandbox} from 'microsandbox';
import {z} from 'zod';
import {registerSandboxTools} from 'microsandbox-mcp/dist/tools/sandbox.js';
import {registerExecTools} from 'microsandbox-mcp/dist/tools/exec.js';
import {ok,fail} from 'microsandbox-mcp/dist/utils/response.js';
import {removeOwned,accessOwned} from './cleanup.mjs';
import {sandboxHandleData,sandboxSummaryData} from 'microsandbox-mcp/dist/utils/serialization.js';
import {formatExecOutput} from 'microsandbox-mcp/dist/utils/exec-output.js';
import {readFileSync} from 'node:fs';
const {version}=JSON.parse(readFileSync(new URL('./package.json',import.meta.url),'utf8'));
const server=new McpServer({name:'labby-microsandbox',version});
const facade={registerTool(name,config,callback){
  if(['sandbox_exec','sandbox_inspect','sandbox_list'].includes(name))return server.registerTool(name,{
    ...config,inputSchema:config.inputSchema.extend({expectedOwner:z.string().uuid().optional()}),
    _meta:{...config._meta,'labby.tailcat.owned_access':1},
  },async args=>{
    if(args.expectedOwner===undefined)return callback(args);
    try{
      const result=await accessOwned(name,args,Sandbox,{inspect:sandboxHandleData,summary:sandboxSummaryData},formatExecOutput);
      if(args.treatNonZeroAsError&&!result.data.success)return fail('exec_failed','Command failed',{details:result.data});
      return ok(result.data,{truncated:result.truncated});
    }catch{return fail('owned_access_failed','Owned sandbox access failed.');}
  });
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
registerExecTools(facade);
await server.connect(new StdioServerTransport());
