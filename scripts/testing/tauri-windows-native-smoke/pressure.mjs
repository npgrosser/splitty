// Fixture-only concurrent workload: never infer health from polls after upload.
export async function observePressure(work) {
 const chronology={started:work.now(),apiStarted:null,firstApiCompleted:null,lastApiCompleted:null,apiReplies:0,apiRepliesWhileStalled:0,fileStarted:null,fileCompleted:null,fileCompletedBeforeSlowClose:false,outputStarted:null,outputCompleted:null,postCloseApiStarted:null,postCloseApiCompleted:null};
 let high=await work.sample();
 const check=()=>{if(!work.alive())throw Error('healthy native connection changed or disconnected')};
 const api=async()=>{
  const deadline=work.now()+45000;
  do{
   check();chronology.apiStarted??=work.now();await work.request();check();
   const at=work.now();chronology.firstApiCompleted??=at;chronology.lastApiCompleted=at;chronology.apiReplies++;
   if(work.pausedPackets()>0&&!work.closed())chronology.apiRepliesWhileStalled++;
   const current=await work.sample();if(current.bytes>high.bytes)high=current;
   if(work.closed())break;await work.wait(100);
  }while(!work.closed()&&work.now()<deadline);
 };
 const file=async()=>{check();chronology.fileStarted=work.now();const path=await work.upload();check();chronology.fileCompleted=work.now();chronology.fileCompletedBeforeSlowClose=!work.closed();return path};
 const output=async()=>{chronology.outputStarted=work.now();await work.output();check();chronology.outputCompleted=work.now()};
 try{
  // Each task begins before awaiting any other task's completion.
  const [healthyFile]=await Promise.all([file(),output(),api()]);
  if(!work.closed()||work.pausedPackets()===0)throw Error('non-consuming client was not independently closed');
  if(chronology.apiReplies===0)throw Error('healthy API did not complete');
  chronology.firstPausedAt=work.firstPausedAt?.();chronology.closedAt=work.closedAt?.();
  check();chronology.postCloseApiStarted=work.now();await work.request();check();chronology.postCloseApiCompleted=work.now();
  return {healthyFile,chronology,high};
 }catch(error){chronology.firstPausedAt=work.firstPausedAt?.();chronology.closedAt=work.closedAt?.();error.pressureChronology=chronology;throw error}
}
