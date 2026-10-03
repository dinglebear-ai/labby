/** The native SDK checks the persisted instance identity inside destroy(). */
export async function removeOwned(args, sdk) {
  const {name,expectedOwner,force=false}=args;
  if(Object.keys(args).some(key=>!['name','expectedOwner','force'].includes(key))
    ||!/^labby-tailcat-[a-f0-9]{32}$/.test(name||'')
    ||!/^[-a-f0-9]{36}$/.test(expectedOwner||'')||typeof force!=='boolean')throw Error('Invalid owned cleanup');
  const handle=await sdk.get(name);
  if(handle.name!==name||handle.config()?.labels?.['labby-tailcat-owner']!==expectedOwner)throw Error('Ownership changed');
  // Do not resolve by name again. This handle rejects a same-name replacement.
  await handle.destroy({force,timeoutMs:10000});
  return [{name,removed:true}];
}

/** Resolve once, validate ownership, and retain the SDK instance-bound handle. */
async function ownedHandle(name, owner, sdk) {
  if(!/^labby-tailcat-[a-f0-9]{32}$/.test(name||'') || !/^[-a-f0-9]{36}$/.test(owner||''))throw Error('Invalid owned access');
  const handle=await sdk.get(name);
  if(handle.name!==name || handle.config()?.labels?.['labby-tailcat-owner']!==owner)throw Error('Ownership changed');
  return handle;
}
export async function accessOwned(tool,args,sdk,serialize,formatOutput) {
  if(tool==='sandbox_list') {
    if(!Array.isArray(args.names)||args.names.length>8)throw Error('Invalid owned list');
    const result=[];
    for(const name of args.names) {
      const handle=await ownedHandle(name,args.expectedOwner,sdk);
      if(!args.status||args.status==='all'||handle.status===args.status)result.push(serialize.summary(handle));
    }
    return {data:result};
  }
  const handle=await ownedHandle(args.name,args.expectedOwner,sdk);
  if(tool==='sandbox_inspect')return {data:serialize.inspect(handle)};
  if(tool!=='sandbox_exec')throw Error('Unsupported owned access');
  const wasRunning=handle.status==='running';
  if(!wasRunning&&!args.startIfStopped)throw Error('Sandbox is not running');
  // connectOrStart is documented to reject a same-name replacement. It retains
  // the exact checked instance through connection/start and command dispatch.
  const sandbox=wasRunning?await handle.connect():await handle.connectOrStart({detached:true});
  try {
    const output=await sandbox.execWith(args.command,b=>{
      if(args.args)b=b.args(args.args);
      if(args.cwd??args.workdir)b=b.cwd(args.cwd??args.workdir);
      if(args.env)b=b.envs(args.env);
      if(args.user)b=b.user(args.user);
      if(args.tty!==undefined)b=b.tty(args.tty);
      const timeout=args.timeoutMs??(args.timeout?args.timeout*1000:undefined);
      if(timeout)b=b.timeout(timeout);
      for(const r of args.rlimits??[])b=b.rlimitRange(r.resource,r.soft,r.hard??r.soft);
      if(args.stdinBase64!==undefined)b=b.stdinBytes(Buffer.from(args.stdinBase64,'base64'));
      else if(args.stdin!==undefined)b=b.stdinBytes(Buffer.from(args.stdin,'utf8'));
      return b;
    });
    return formatOutput(output,args.maxBytes);
  } finally {
    // Stop only the originally checked handle, never a later name lookup.
    if(!wasRunning&&!args.keepRunning)await handle.stopWithTimeout(10000);
  }
}
