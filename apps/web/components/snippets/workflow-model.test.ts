import test from 'node:test'
import assert from 'node:assert/strict'
import { runInNewContext } from 'node:vm'
import { generateWorkflowCode, readWorkflowPlan, renameWorkflowStep, previewWorkflow, suggestWorkflowMapping, validateWorkflow, type WorkflowPlan, type WorkflowStep } from './workflow-model'
import { ownPath } from './workflow-selectors'
const step = (id: string, mapping: Record<string, unknown> = {}, dependsOn: string[] = []): WorkflowStep => ({ id, tool: `test::${id}`, mapping, dependsOn })
const plan = (...steps: WorkflowStep[]): WorkflowPlan => ({ version: 1, steps })

test('waves preserve source order and infer recursively nested dependencies', () => {
 const result = validateWorkflow(plan(step('last', { values: [{ q: '$steps.first.rows.0.name' }] }), step('other'), step('first')))
 assert.equal(result.valid, true); assert.deepEqual(result.waves, [['other', 'first'], ['last']]); assert.deepEqual(result.dependencies.last, ['first'])
})
test('cycles, duplicates, dangling refs, unsafe selectors and non-JSON mappings fail before generation', () => {
 const circular: Record<string, unknown> = {}; circular.self = circular
 const invalid = [plan(step('a'), step('a')), plan(step('a', {}, ['missing'])), plan(step('a', {}, ['b']), step('b', {}, ['a'])), plan(step('a', { q: '$steps.a.x' })), plan(step('a', { q: '$steps.missing.x' })), plan(step('a', circular)), plan(step('a', { q: undefined })), plan(step('a', JSON.parse('{"__proto__":{"x":1}}')))]
 for (const value of ['$input.__proto__.x','$steps.a.constructor.x','$input.x.prototype','$input.','$steps..x']) invalid.push(plan(step('a',{q:value})))
 for (const value of invalid) { assert.equal(validateWorkflow(value).valid,false); assert.throws(()=>generateWorkflowCode(value)) }
})
test('lookup accepts array-index paths but refuses inherited or unsafe properties',()=>{
 assert.equal(ownPath({rows:[{name:'ok'}]},['rows','0','name']),'ok')
 for(const path of [['toString'],['__proto__'],['rows','4']]) assert.throws(()=>ownPath({rows:[]},path))
})
test('static preview resolves inputs and leaves runtime output expressions visible',()=>{
 const result=previewWorkflow(plan(step('a',{q:['$input.query','$input.missing']}),step('b',{q:'$steps.a.rows.0.name'})),{query:'hello'})
 assert.deepEqual(result.steps[0].params,{q:['hello','$input.missing']});assert.deepEqual(result.steps[0].unresolved,['$input.missing']);assert.deepEqual(result.steps[1].unresolved,['$steps.a.rows.0.name']);assert.equal(result.steps[1].wave,1)
})
test('schema suggestions require unique same-name scalar compatibility',()=>{
 const target={type:'object',properties:{query:{type:'string'},count:{type:'number'}}}
 assert.deepEqual(suggestWorkflowMapping(target,{query:{type:'string'},count:{type:'integer'}}),{query:'$input.query',count:'$input.count'})
 const prior=[{id:'search',outputSchema:{type:'object',properties:{data:{type:'object',properties:{query:{type:'string'}}}}}}]
 assert.deepEqual(suggestWorkflowMapping(target,{},prior),{query:'$steps.search.data.query'})
 assert.deepEqual(suggestWorkflowMapping(target,{query:{type:'string'}},prior),{})
 assert.deepEqual(suggestWorkflowMapping({properties:{query:{type:'string',minLength:3}}},{query:{type:'string'}}),{})
})
interface FixtureResult {steps:{id:string;status:string;value?:unknown;dependencies?:string[]}[];all_ok:boolean;ok:boolean}
async function execute(plan:WorkflowPlan,input:unknown,callTool:(name:string,params:Record<string,unknown>)=>Promise<unknown>){
 const waves:number[]=[]
 const codemode={batch:async(jobs:(()=>Promise<unknown>)[])=>{waves.push(jobs.length);const settled=await Promise.allSettled(jobs.map(async(job)=>job()));return {ok:settled.flatMap((entry,i)=>entry.status==='fulfilled'?[{i,value:entry.value}]:[]),failed:settled.flatMap((entry,i)=>entry.status==='rejected'?[{i,error:entry.reason}]:[])}}}
 const source=generateWorkflowCode(plan);assert.doesNotMatch(source,/Promise\.all/)
 const runnable=runInNewContext(`(${source})`,{input,codemode,callTool}) as ()=>Promise<FixtureResult>
 return {result:JSON.parse(JSON.stringify(await runnable())) as FixtureResult,waves}
}
test('generated JS runs concurrent waves, resolves outputs and keeps stable source order',async()=>{
 const invoked:string[]=[];let active=0;let maximum=0
 const {result,waves}=await execute(plan(step('consume',{values:['$steps.search.rows.0.name','$input.query']}),step('search'),step('independent')),{query:'q'},async(name,params)=>{invoked.push(name);active++;maximum=Math.max(maximum,active);await new Promise(resolve=>setTimeout(resolve,5));active--;return name==='test::search'?{rows:[{name:'result'}]}:params})
 assert.equal(maximum,2);assert.deepEqual(waves,[2,1]);assert.deepEqual(invoked,['test::search','test::independent','test::consume']);assert.deepEqual(result.steps.map(entry=>[entry.id,entry.status]),[['consume','succeeded'],['search','succeeded'],['independent','succeeded']]);assert.deepEqual(result.steps[0].value,{values:['result','q']});assert.equal(result.all_ok,true);assert.equal(result.ok,true)
})
test('generated JS preserves siblings and skips failed prerequisites transitively',async()=>{
 const invoked:string[]=[]
 const {result,waves}=await execute(plan(step('fail'),step('ok'),step('child',{q:'$steps.fail.value'}),step('grandchild',{},['child']),step('missing',{q:'$steps.ok.absent'}),step('afterMissing',{},['missing'])),{},async(name)=>{invoked.push(name);if(name==='test::fail')throw {kind:'upstream',message:'fixture failure'};return {value:'kept'}})
 assert.deepEqual(waves,[2,1]);assert.deepEqual(invoked,['test::fail','test::ok']);assert.deepEqual(result.steps.map(entry=>entry.status),['failed','succeeded','skipped','skipped','failed','skipped']);assert.deepEqual(result.steps[2].dependencies,['fail']);assert.deepEqual(result.steps[1].value,{value:'kept'});assert.equal(result.all_ok,false);assert.equal(result.ok,false)
})

test('checked-in hardened runtime fixture exactly matches the workflow generator', async()=>{
 const {readFile}=await import('node:fs/promises')
 const source=await readFile(new URL('../../../../crates/labby/tests/fixtures/workflow-builder.js',import.meta.url),'utf8')
 assert.equal(source,generateWorkflowCode(plan(
  {id:'lookup',tool:'fixture::lookup',mapping:{query:'$input.query'},dependsOn:[]},
  {id:'independent',tool:'fixture::independent',mapping:{},dependsOn:[]},
  {id:'consume',tool:'fixture::consume',mapping:{id:'$steps.lookup.items.0.id'},dependsOn:[]},
  {id:'blocked',tool:'fixture::blocked',mapping:{},dependsOn:['consume']},
 )))
})
test('preview masks nested secrets, target schemas and sensitive input aliases',()=>{
 const preview=previewWorkflow(plan(step('a',{value:'$input.api_token',data:'$input.payload',password:'literal',opaque:'literal'})),{api_token:'do-not-show',payload:{nested:{authorization:'hidden',visible:'ok'}}},{a:{properties:{opaque:{writeOnly:true}}}})
 assert.deepEqual(preview.steps[0].params,{value:'[redacted]',data:{nested:{authorization:'[redacted]',visible:'ok'}},password:'[redacted]',opaque:'[redacted]'})
 assert.deepEqual(suggestWorkflowMapping({properties:{api_token:{type:'string'},opaque:{type:'string',format:'password'}}},{api_token:{type:'string'},opaque:{type:'string'}}),{})
})


test('builder recognition roundtrips bare source and Markdown without evaluating source',()=>{
 const original=plan(step('lookup',{query:'$input.query'}),step('consume',{id:'$steps.lookup.items.0.id'}))
 const source=generateWorkflowCode(original)
 assert.deepEqual(readWorkflowPlan(source),original)
 assert.deepEqual(readWorkflowPlan('---\nname: demo\n---\n```js\n'+source+'\n```\n'),original)
 assert.deepEqual(readWorkflowPlan('---\nname: demo\ndescription: Search and consume\ntools: ["test::lookup", "test::consume"]\n---\n\n# demo\n\nSearch and consume\n\n```js\n'+source+'\n```\n'),original)
 assert.equal(readWorkflowPlan('```json\n{}\n```\n```js\n'+source+'\n```'),undefined)
 assert.equal(readWorkflowPlan(source.replace('callTool(step.tool, resolve(step.mapping))',"callTool('other::tool', {})")),undefined)
 assert.equal(readWorkflowPlan(source.replace('const plan = {','const plan = malformed{')),undefined)
 assert.equal(readWorkflowPlan(source.replace('items.0.id','__proto__.id')),undefined)
 assert.equal(readWorkflowPlan(source.replace('"waves":[["lookup"],["consume"]]','"waves":[["consume"],["lookup"]]')),undefined)
 assert.equal(readWorkflowPlan('```js\n'+source+'\n```\n```js\n'+source+'\n```'),undefined)
 assert.equal(readWorkflowPlan('async()=>{throw new Error("never execute") }'),undefined)
})


test('whole-result selectors infer dependencies and preserve structured output values',async()=>{
 const {result,waves}=await execute(plan(step('lookup'),step('consume',{payload:{result:'$steps.lookup'}})),{},async(name,params)=>name==='test::lookup'?{nested:{items:[1,2]}}:params)
 assert.deepEqual(waves,[1,1]);assert.deepEqual(result.steps[1].value,{payload:{result:{nested:{items:[1,2]}}}});assert.equal(result.ok,true)
})


test('renaming rewrites nested selectors and explicit edges without changing literals or input plan',()=>{
 const original=plan(step('lookup'),step('other'),step('consume',{payload:[{whole:'$steps.lookup',field:'$steps.lookup.rows.0.id'}],literal:'Text $steps.lookup.rows',unrelated:'$steps.other.value'},['lookup','other']))
 const snapshot=JSON.stringify(original)
 const renamed=renameWorkflowStep(original,'lookup','search')
 assert.equal(JSON.stringify(original),snapshot)
 assert.equal(renamed.steps[0].id,'search')
 assert.deepEqual(renamed.steps[2].dependsOn,['search','other'])
 assert.deepEqual(renamed.steps[2].mapping,{payload:[{whole:'$steps.search',field:'$steps.search.rows.0.id'}],literal:'Text $steps.lookup.rows',unrelated:'$steps.other.value'})
 assert.deepEqual(renameWorkflowStep(renamed,'search','lookup'),original)
 assert.deepEqual(renameWorkflowStep(original,'lookup','lookup'),original)
 assert.equal(validateWorkflow(renamed).valid,true)
})
test('renaming rejects unknown ids, unsafe ids and duplicate identities',()=>{
 const original=plan(step('lookup'),step('other'))
 assert.throws(()=>renameWorkflowStep(original,'missing','safe'),/Unknown step/)
 assert.throws(()=>renameWorkflowStep(original,'lookup','other'),/Duplicate/)
 for(const id of ['__proto__','constructor','prototype','bad.id','','bad id']) assert.throws(()=>renameWorkflowStep(original,'lookup',id),/safe/)
})
