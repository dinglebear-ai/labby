const encoder=new TextEncoder();
const MAX=1024*1024,HEADER_MAX=16*1024;
const failure=()=>new Error('Invalid or oversized HTTP response');
const join=(a,b)=>{const out=new Uint8Array(a.length+b.length);out.set(a);out.set(b,a.length);return out};
const index=(bytes,text)=>{
 const needle=encoder.encode(text);
 outer:for(let i=0;i<=bytes.length-needle.length;i++){for(let j=0;j<needle.length;j++)if(bytes[i+j]!==needle[j])continue outer;return i}return -1;
};

/** Incremental response parser for the restricted JSON/SSE MCP endpoint. */
export class HttpDecoder {
 constructor(){this.buffer=new Uint8Array();this.headers=null;this.status=0;this.done=false;this.messages=[];this.messageBytes=0;this.body=[];this.bodyBytes=0;this.remaining=null;this.chunkRemaining=null;this.chunkTerminator=false;this.trailers=false;this.sseParts=[];this.sseLines=[];this.sseBytes=0;this.text=new TextDecoder('utf-8',{fatal:true});}
 push(bytes){
  if(this.done){if(bytes.length)throw failure();return}
  if(this.buffer.length+bytes.length>MAX+HEADER_MAX)throw failure();
  this.buffer=join(this.buffer,bytes);
  if(!this.headers){
   const pos=index(this.buffer,'\r\n\r\n');
   if(pos<0){if(this.buffer.length>HEADER_MAX)throw failure();return}
   if(pos>HEADER_MAX)throw failure();
   const lines=new TextDecoder().decode(this.buffer.slice(0,pos)).split('\r\n');
   const match=/^HTTP\/1\.[01] ([0-9]{3})(?: |$)/.exec(lines.shift());if(!match)throw failure();this.status=Number(match[1]);this.headers={};
   for(const line of lines){const colon=line.indexOf(':');if(colon<=0)throw failure();const key=line.slice(0,colon).toLowerCase(),value=line.slice(colon+1).trim();if(!/^[a-z0-9-]+$/.test(key)||key in this.headers)throw failure();this.headers[key]=value;}
   if(this.headers['content-encoding']&&this.headers['content-encoding']!=='identity')throw failure();
   if(this.headers['transfer-encoding']&&this.headers['transfer-encoding']!=='chunked')throw failure();
   if(this.headers['transfer-encoding']&&this.headers['content-length'])throw failure();
   if(this.headers['content-length']!==undefined){if(!/^[0-9]+$/.test(this.headers['content-length']))throw failure();this.remaining=Number(this.headers['content-length']);if(!Number.isSafeInteger(this.remaining)||this.remaining>MAX)throw failure();}
   this.streaming=(this.headers['content-type']||'').split(';')[0].trim()==='text/event-stream';
   this.buffer=this.buffer.slice(pos+4);
   if(this.status===204||this.status===202&&this.remaining===0){this.complete();return}
  }
  if(this.headers['transfer-encoding']==='chunked')this.chunks();
  else if(this.remaining!==null){const n=Math.min(this.buffer.length,this.remaining);this.consume(this.buffer.slice(0,n));this.buffer=this.buffer.slice(n);this.remaining-=n;if(!this.remaining)this.complete();}
  else{this.consume(this.buffer);this.buffer=new Uint8Array();}
 }
 chunks(){
  while(!this.done){
   if(this.trailers){const pos=index(this.buffer,'\r\n');if(pos<0)return;if(pos!==0)throw failure();this.buffer=this.buffer.slice(2);this.complete();return}
   if(this.chunkRemaining===null){const pos=index(this.buffer,'\r\n');if(pos<0){if(this.buffer.length>1024)throw failure();return}const hex=new TextDecoder().decode(this.buffer.slice(0,pos));if(!/^[0-9a-fA-F]{1,8}$/.test(hex))throw failure();this.chunkRemaining=parseInt(hex,16);if(this.chunkRemaining>MAX)throw failure();this.buffer=this.buffer.slice(pos+2);if(!this.chunkRemaining){this.trailers=true;continue}}
   if(this.chunkRemaining){const n=Math.min(this.buffer.length,this.chunkRemaining);this.consume(this.buffer.subarray(0,n));this.buffer=this.buffer.subarray(n);this.chunkRemaining-=n;if(this.chunkRemaining)return;this.chunkTerminator=true;}
   if(this.chunkTerminator){if(this.buffer.length&&this.buffer[0]!==13)throw failure();if(this.buffer.length<2)return;if(this.buffer[1]!==10)throw failure();this.buffer=this.buffer.subarray(2);this.chunkTerminator=false;this.chunkRemaining=null;}
  }
 }
 consume(bytes){
  if(this.streaming){
   const lines=this.text.decode(bytes,{stream:true}).split('\n');
   for(let i=0;i<lines.length;i++){
    this.sseBytes+=encoder.encode(lines[i]).length+(i<lines.length-1?1:0);if(this.sseBytes>MAX)throw failure();
    this.sseParts.push(lines[i]);if(i===lines.length-1)break;
    const line=this.sseParts.join('').replace(/\r$/,'');this.sseParts=[];
    if(line){this.sseLines.push(line);continue}
    const data=this.sseLines.filter(l=>l.startsWith('data:')).map(l=>l.slice(5).replace(/^ /,'')).join('\n');this.sseLines=[];this.sseBytes=0;
    if(data){this.messageBytes+=encoder.encode(data).length;if(this.messageBytes>MAX)throw failure();this.messages.push(JSON.parse(data));}
   }
  }else{this.bodyBytes+=bytes.length;if(this.bodyBytes>MAX)throw failure();if(bytes.length)this.body.push(bytes);}
 }
 complete(){
  if(this.buffer.length)throw failure();
  if(!this.streaming&&this.bodyBytes){let bytes=new Uint8Array(this.bodyBytes),offset=0;for(const part of this.body){bytes.set(part,offset);offset+=part.length;}this.messages.push(JSON.parse(this.text.decode(bytes)));this.body=[];}
  this.done=true;
 }
 finish(){if(this.done)return;if(!this.headers||this.headers['transfer-encoding']||this.remaining!==null&&this.remaining!==0)throw failure();this.complete();}
 takeMessages(){const messages=this.messages;this.messages=[];this.messageBytes=0;return messages;}
}
