// Only presentation preferences are stored. Gateway snapshots and credentials
// never enter this record, and the LiveView validates every restored identity.
const storageKey=key=>typeof key==='string'&&/^[a-f0-9]{64}$/.test(key)?`labby:projects:${key}:v1`:null
const text=(value,max)=>typeof value==='string'&&value.length<=max&&!/[\u0000-\u001f\u007f-\u009f]/u.test(value)
function bounded(value){
 if(!value||typeof value!=='object'||Array.isArray(value))return {}
 const result={}
 if(['list','grid','table'].includes(value.layout))result.layout=value.layout
 if(text(value.query,200))result.query=value.query
 if(text(value.team,128))result.team=value.team
 if(value.current===null||text(value.current,344))result.current=value.current
 return result
}
export function readPreferences(storage,key){
 const name=storageKey(key)
 if(!name)return {}
 try{const raw=storage.getItem(name);if(!raw||raw.length>4096)return {};return bounded(JSON.parse(raw))}catch{return {}}
}
export function writePreferences(storage,key,value){
 const name=storageKey(key)
 if(!name)return false
 try{storage.setItem(name,JSON.stringify(bounded(value)));return true}catch{return false}
}
export default {
 mounted(){
  const key=this.el.dataset.preferenceKey
  this.key=key
  let storage
  try{storage=window.localStorage}catch{}
  this.handleEvent('projects-preferences-state',payload=>{
   if(payload.key===this.key)writePreferences(storage,this.key,payload.preferences)
  })
  this.pushEvent('projects_preferences',{preferences:readPreferences(storage,key)})
 }
}
