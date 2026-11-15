// Only card order/width are persisted; gateway records and identity stay server-side.
const cards=['traffic','attention','inventory','rankings','latency','callers','runtime','clients','upstreams','fleet','calls']
const telemetry=['traffic','rankings','latency','calls']
const storageKey=key=>typeof key==='string'&&/^[a-f0-9]{64}$/.test(key)?`labby:overview:${key}:v1`:null
function bounded(value){
 if(!value||typeof value!=='object'||Array.isArray(value)||!Array.isArray(value.order)||value.order.length>64)return {}
 return {order:[...new Set(value.order.filter(id=>cards.includes(id)))],widths:Object.fromEntries(Object.entries(value.widths&&typeof value.widths==='object'&&!Array.isArray(value.widths)?value.widths:{}).filter(([id,wide])=>telemetry.includes(id)&&typeof wide==='boolean'))}
}
export function readLayout(storage,key){const name=storageKey(key);if(!name)return {};try{const raw=storage.getItem(name);return raw&&raw.length<=4096?bounded(JSON.parse(raw)):{}}catch{return {}}}
export function writeLayout(storage,key,value){const name=storageKey(key);if(!name)return false;try{storage.setItem(name,JSON.stringify(bounded(value)));return true}catch{return false}}
export default {
 mounted(){
  this.key=this.el.dataset.preferenceKey
  try{this.storage=window.localStorage}catch{}
  this.handleEvent('overview-layout-state',payload=>{if(payload.key===this.key&&!writeLayout(this.storage,this.key,payload.layout))this.pushEvent('overview_storage_error',{})})
  this.handleEvent('overview-focus-card',({id})=>{if(cards.includes(id))document.getElementById(`ov-card-${id}`)?.focus()})
  this.pushEvent('overview_preferences',readLayout(this.storage,this.key))
  this.onDragStart=event=>{
   const handle=event.target.closest('[data-overview-handle]')
   const card=handle?.closest('[data-overview-card]')
   if(!card||this.el.dataset.editing!=='true'){event.preventDefault();return}
   this.dragging=card.dataset.overviewCard
   event.dataTransfer.effectAllowed='move';event.dataTransfer.setData('text/plain',this.dragging)
  }
  this.onDragOver=event=>{if(this.dragging&&event.target.closest('[data-overview-card]'))event.preventDefault()}
  this.onDrop=event=>{
   const card=event.target.closest('[data-overview-card]')
   if(this.dragging&&card){event.preventDefault();const box=card.getBoundingClientRect();this.pushEvent('overview_place',{source:this.dragging,target:card.dataset.overviewCard,after:event.clientY>box.top+box.height/2})}
   this.dragging=null
  }
  this.onDragEnd=()=>{this.dragging=null}
  this.onKey=event=>{
   const card=event.target.closest('[data-overview-card]')
   if(event.altKey&&card&&['ArrowUp','ArrowDown'].includes(event.key)){
    event.preventDefault();this.pushEvent('overview_move',{id:card.dataset.overviewCard,direction:event.key==='ArrowUp'?'earlier':'later'})
   }
  }
  for(const [name,fn] of [['dragstart',this.onDragStart],['dragover',this.onDragOver],['drop',this.onDrop],['dragend',this.onDragEnd],['keydown',this.onKey]])this.el.addEventListener(name,fn)
 },
 updated(){if(this.key!==this.el.dataset.preferenceKey){this.key=this.el.dataset.preferenceKey;this.dragging=null;this.pushEvent('overview_preferences',readLayout(this.storage,this.key))}},
 destroyed(){for(const [name,fn] of [['dragstart',this.onDragStart],['dragover',this.onDragOver],['drop',this.onDrop],['dragend',this.onDragEnd],['keydown',this.onKey]])this.el.removeEventListener(name,fn)}
}
