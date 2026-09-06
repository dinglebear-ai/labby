import assert from 'node:assert/strict'
import test from 'node:test'
import {__setBrowserSessionStateForTests} from '../auth/session-store.ts'
import {createProject,listProjects} from './client.ts'
test('project lifecycle uses authoritative endpoint and csrf for mutation',async()=>{__setBrowserSessionStateForTests({status:'authenticated',user:{sub:'owner'},expiresAt:Date.now()+1000,csrfToken:'csrf',isAdmin:false});const requests:Request[]=[];globalThis.fetch=async(input,init)=>{const request=new Request(new URL(String(input),'http://labby.test'),init);requests.push(request);return Response.json(requests.length===1?[]:{project_id:'p'})};await listProjects();await createProject('team','p','Project');assert.equal(requests[0]?.headers.get('x-csrf-token'),null);assert.equal(requests[1]?.headers.get('x-csrf-token'),'csrf');assert.deepEqual(JSON.parse(await requests[1]!.text()),{action:'projects.create',params:{team_id:'team',project_id:'p',name:'Project'}})})
test('project denial is not rendered as an empty list',async()=>{globalThis.fetch=async()=>Response.json({}, {status:403});await assert.rejects(listProjects(),/failed \(403\)/)})
