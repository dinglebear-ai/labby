import {test} from 'node:test';
import assert from 'node:assert/strict';
import {removeOwned} from './cleanup.mjs';
const owner='01234567-89ab-4def-8123-456789abcdef';
const name='labby-tailcat-0123456789abcdef0123456789abcdef';
test('cleanup rejects foreign ownership before any lifecycle operation',async()=>{
 let called=false;
 await assert.rejects(removeOwned({name,expectedOwner:owner,force:true},{get:async()=>({name,config:()=>({labels:{'labby-tailcat-owner':'foreign'}}),destroy:async()=>{called=true;}})}));
 assert.equal(called,false);
});
test('cleanup uses one identity-bound handle and refuses a concurrent replacement',async()=>{
 let current='original',lookups=0,destroyed=false;
 const sdk={get:async()=>{lookups++;return {name,id:'original',config:()=>{current='replacement';return {labels:{'labby-tailcat-owner':owner}};},destroy:async()=>{if(current!=='original')throw Error('identity changed');destroyed=true;}};}};
 await assert.rejects(removeOwned({name,expectedOwner:owner,force:true},sdk));
 assert.equal(lookups,1);assert.equal(destroyed,false);
});
test('owned cleanup destroys exact instance and refuses bulk selectors',async()=>{
 let destroyed=0;
 const sdk={get:async()=>({name,id:'original',config:()=>({labels:{'labby-tailcat-owner':owner}}),destroy:async()=>{destroyed++;}})};
 await removeOwned({name,expectedOwner:owner,force:true},sdk);assert.equal(destroyed,1);
 await assert.rejects(removeOwned({name,expectedOwner:owner,names:['foreign']},sdk));assert.equal(destroyed,1);
});
