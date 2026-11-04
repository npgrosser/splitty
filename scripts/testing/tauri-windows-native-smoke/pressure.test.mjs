import assert from 'node:assert/strict';
import {observePressure} from './pressure.mjs';
function fixture({lateFile=false,immediateClose=false,disconnect=false,requestFails=false}={}){
 let closed=false,packets=0,alive=true,at=0,releaseFile;
 const delayed=new Promise(resolve=>{releaseFile=resolve});
 return {now:()=>++at,closed:()=>closed,pausedPackets:()=>packets,alive:()=>alive,
  sample:async()=>({bytes:0}),output:async()=>{packets=1;if(immediateClose){closed=true;releaseFile()}},
  upload:async()=>{if(lateFile)await delayed;return '/private/file'},
  request:async()=>{if(disconnect)alive=false;if(requestFails)throw Error('healthy request failed')},
  wait:async()=>{closed=true;releaseFile()},
  firstPausedAt:()=>1,closedAt:()=>closed?at:undefined};
}
// One real overlapping reply suffices; no arbitrary post-upload poll count.
const success=await observePressure(fixture());
assert.equal(success.chronology.apiRepliesWhileStalled,1);
assert.equal(success.chronology.fileCompletedBeforeSlowClose,true);
assert.equal(success.healthyFile,'/private/file');
assert.ok(success.chronology.postCloseApiCompleted>=success.chronology.postCloseApiStarted);
// Closing an exhausted slow peer immediately is valid. The unchanged healthy
// connection must finish its transfer and explicitly answer after that close.
const immediate=await observePressure(fixture({immediateClose:true,lateFile:true}));
assert.equal(immediate.chronology.apiRepliesWhileStalled,0);
assert.equal(immediate.chronology.fileCompletedBeforeSlowClose,false);
assert.equal(immediate.healthyFile,'/private/file');
assert.equal(typeof immediate.chronology.postCloseApiCompleted,'number');
const late=await observePressure(fixture({lateFile:true}));
assert.equal(late.chronology.fileCompletedBeforeSlowClose,false);
await assert.rejects(observePressure(fixture({disconnect:true})),/healthy native connection changed/);
await assert.rejects(observePressure(fixture({requestFails:true})),error=>{
 assert.match(error.message,/healthy request failed/);assert.equal(typeof error.pressureChronology.started,'number');return true;
});
console.log('PASS overlapping/late/immediate-close healthy transfers; changed or failed healthy connection rejected');
