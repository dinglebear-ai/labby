import {writeFileSync} from 'node:fs'
export class BoundedDiagnostics {
 constructor(path,secrets=[],limit=262144){this.path=path;this.secrets=secrets;this.limit=limit;this.text=''}
 write(chunk){this.text=(this.text+chunk.toString()).slice(-this.limit*2);this.flush()}
 flush(){let text=this.text;for(const secret of this.secrets)if(secret)text=text.split(secret).join('[REDACTED]');text=text.replace(/(authorization\s*[:=]\s*(?:bearer\s+)?)[^\r\n]+/gi,'$1[REDACTED]').replace(/((?:token|password|credential|secret)["']?\s*[:=]\s*)[^\s,}]+/gi,'$1[REDACTED]');let bytes=Buffer.from(text);if(bytes.length>this.limit)bytes=bytes.subarray(bytes.length-this.limit);writeFileSync(this.path,bytes,{mode:0o600})}
}
export async function waitForHttp(url,{stage='ready',attempts=120,delayMs=250,timeoutMs=1000}={}){
 for(let i=0;i<attempts;i++){try{const response=await fetch(url,{signal:AbortSignal.timeout(timeoutMs)});if(response.ok)return}catch{}if(i+1<attempts)await new Promise(r=>setTimeout(r,delayMs))}
 throw Object.assign(new Error(`${stage}: readiness deadline exceeded (${url}); inspect retained diagnostics before retrying`),{code:'E2E_READY_TIMEOUT',stage,attempts,url})
}
