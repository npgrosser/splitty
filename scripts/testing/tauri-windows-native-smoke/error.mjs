// Fixture diagnostics only: preserve useful structured failures without secrets.
export function fixtureError(value) {
 const seen=new Set();
 const redact=text=>String(text).replace(/\b[0-9a-f]{32,}\b/gi,'[redacted-id]').slice(0,2048);
 const clean=(item,depth)=>{
  if(typeof item==='string')return redact(item);
  if(item===null||typeof item==='number'||typeof item==='boolean')return item;
  if(typeof item!=='object')return redact(String(item));
  if(depth>2||seen.has(item))return '[bounded]';seen.add(item);
  const out={};
  for(const key of Object.getOwnPropertyNames(item).slice(0,16)){
   if(/token|auth|proof|password|secret|credential|key/i.test(key)){out[key]='[redacted]';continue}
   if(!['name','message','hint','code','status','error','cause'].includes(key)){out[key]='[omitted]';continue}
   try{out[key]=clean(item[key],depth+1)}catch{out[key]='[unreadable]'}
  }
  return out;
 };
 try{return (typeof value==='string'?redact(value):JSON.stringify(clean(value,0))).slice(0,4096)}catch{return 'Unserializable fixture error'}
}
