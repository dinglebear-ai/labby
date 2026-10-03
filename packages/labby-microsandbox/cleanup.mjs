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
