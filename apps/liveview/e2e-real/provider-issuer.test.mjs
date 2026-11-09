import {test} from 'node:test'
import assert from 'node:assert/strict'
import {request} from 'node:https'
import {createServer} from 'node:net'
import {createHash, createPublicKey, verify} from 'node:crypto'
import {rmSync} from 'node:fs'
import {prepareProviderTls,startProviderIssuer} from './provider-issuer.mjs'

function call(issuer,path,body){return new Promise((resolve,reject)=>{const req=request(`${issuer.origin}${path}`,{ca:issuer.ca,method:body?'POST':'GET',headers:body?{'content-type':'application/x-www-form-urlencoded'}:{}},res=>{let text='';res.on('data',c=>text+=c);res.on('end',()=>resolve({status:res.statusCode,headers:res.headers,text}))});req.on('error',reject);req.end(body)})}
test('TLS discovery, two identities, signed nonce-bound tokens and one-use PKCE codes',async()=>{
 const issuer=await startProviderIssuer();try{
 const discovery=await call(issuer,'/.well-known/openid-configuration');assert.equal(discovery.status,200);assert.equal(JSON.parse(discovery.text).issuer,issuer.origin)
 const jwks=JSON.parse((await call(issuer,'/jwks.json')).text)
 const verifier='v'.repeat(43);const challenge=createHash('sha256').update(verifier).digest('base64url');const subjects=[]
 for(const account of issuer.accounts){
 const query=new URLSearchParams({client_id:issuer.clientId,redirect_uri:issuer.redirectUri,response_type:'code',scope:'openid email profile',state:'state',nonce:'nonce',code_challenge:challenge,code_challenge_method:'S256'})
 const page=await call(issuer,`/authorize?${query}`);assert.equal(page.status,200)
 const transaction=page.text.match(/name="transaction" value="([a-f0-9]+)"/)[1]
 const login=await call(issuer,'/login',new URLSearchParams({transaction,email:account.email,password:account.password}).toString());assert.equal(login.status,302)
 const code=new URL(login.headers.location).searchParams.get('code')
 const params={grant_type:'authorization_code',client_id:issuer.clientId,client_secret:issuer.clientSecret,redirect_uri:issuer.redirectUri,code,code_verifier:'x'.repeat(43)}
 assert.equal((await call(issuer,'/token',new URLSearchParams(params).toString())).status,400)
 params.code_verifier=verifier
 params.client_secret='invalid-client-secret'
 assert.equal((await call(issuer,'/token',new URLSearchParams(params).toString())).status,400)
 params.client_secret=issuer.clientSecret
 params.redirect_uri='http://127.0.0.1:1/wrong'
 assert.equal((await call(issuer,'/token',new URLSearchParams(params).toString())).status,400)
 params.redirect_uri=issuer.redirectUri
 const response=await call(issuer,'/token',new URLSearchParams(params).toString());assert.equal(response.status,200)
 const token=JSON.parse(response.text).id_token;const [header,payload,signature]=token.split('.');assert.ok(verify('RSA-SHA256',Buffer.from(`${header}.${payload}`),createPublicKey({key:jwks.keys[0],format:'jwk'}),Buffer.from(signature,'base64url')))
 const claims=JSON.parse(Buffer.from(payload,'base64url'));assert.equal(claims.nonce,'nonce');assert.equal(claims.email,account.email);subjects.push(claims.sub)
 assert.equal((await call(issuer,'/token',new URLSearchParams(params).toString())).status,400)
 }
 assert.notEqual(...subjects)
 assert.equal((await call(issuer,'/authorize?client_id=wrong')).status,400)
 assert.equal((await call(issuer,'/login','x'.repeat(9000))).status,413)
 }finally{await issuer.close()}
})

test('launcher-prepared TLS identity is reused and shutdown releases its port',async()=>{
 const tls=prepareProviderTls()
 let issuer
 try{
 issuer=await startProviderIssuer({tlsDirectory:tls.root})
 assert.equal(issuer.spki,tls.spki)
 assert.equal((await call(issuer,'/.well-known/openid-configuration')).status,200)
 const port=new URL(issuer.origin).port
 await issuer.close();issuer=null
 const probe=createServer()
 await new Promise((resolve,reject)=>{probe.once('error',reject);probe.listen(Number(port),'127.0.0.1',resolve)})
 await new Promise(resolve=>probe.close(resolve))
 }finally{if(issuer)await issuer.close();rmSync(tls.root,{recursive:true,force:true})}
})
