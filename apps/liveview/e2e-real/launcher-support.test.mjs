import test from 'node:test'
import assert from 'node:assert/strict'
import {mkdtempSync,readFileSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {BoundedDiagnostics,waitForHttp} from './launcher-support.mjs'

test('diagnostics bound retained bytes and redact credentials and authorization headers', () => {
 const path=join(mkdtempSync(join(tmpdir(),'labby-launcher-unit-')),'process.log')
 const sink=new BoundedDiagnostics(path,['synthetic-private-credential'],256)
 sink.write('Authorization: Bearer other-private-credential\n'+ 'x'.repeat(500))
 sink.write('\nsynthetic-private-credential\n')
 sink.flush()
 const text=readFileSync(path,'utf8')
 assert.ok(Buffer.byteLength(text)<=256)
 assert.ok(!text.includes('private-credential'))
 assert.match(text,/REDACTED/)
})
test('readiness exhausts a bounded budget and reports stage and endpoint',async()=>{
 await assert.rejects(waitForHttp('http://127.0.0.1:1/ready',{stage:'backend_ready',attempts:2,delayMs:1,timeoutMs:10}),error=>error.code==='E2E_READY_TIMEOUT'&&error.stage==='backend_ready'&&error.attempts===2)
})

test('missing backend fails with a stable startup stage instead of a raw process stack',async()=>{
 const {spawnSync}=await import('node:child_process')
 const result=spawnSync(process.execPath,['e2e-real/start.mjs'],{encoding:'utf8',timeout:10000,env:{...process.env,LABBY_E2E_BINARY:'/definitely-missing-labby-e2e',LABBY_E2E_CREDENTIAL:'a'.repeat(64),LABBY_E2E_CONTROL:'b'.repeat(64)}})
 assert.notEqual(result.status,0)
 assert.match(result.stderr,/E2E_BINARY_UNAVAILABLE at binary_admission/)
 assert.ok(!result.stderr.includes('a'.repeat(64)))
})
