/** Real Chrome check, using only Node's fetch/WebSocket and a loopback CDP tab.
 * Usage: node web/browser_test.mjs http://127.0.0.1:9222 http://127.0.0.1:8000/
 * The browser and tools/serve_web.py must already be running. */
import assert from 'node:assert/strict';

function loopback(value) {
  const url=new URL(value);
  if(url.protocol!=='http:' || !['localhost','127.0.0.1','[::1]'].includes(url.hostname)) throw new Error('Checks require loopback HTTP URLs');
  return url;
}
const endpoint=loopback(process.argv[2]??'http://127.0.0.1:9222/');
const viewer=loopback(process.argv[3]??'http://127.0.0.1:8000/');
const target=await (await fetch(new URL('/json/new?about:blank',endpoint),{method:'PUT',signal:AbortSignal.timeout(5000)})).json();
const socket=new WebSocket(target.webSocketDebuggerUrl), pending=new Map(), errors=[]; let id=0;
function command(method,params={}) {
  return new Promise((resolve,reject)=>{
    const current=++id, timer=setTimeout(()=>{pending.delete(current);reject(new Error(`CDP timeout: ${method}`));},45000);
    pending.set(current,{resolve,reject,timer}); socket.send(JSON.stringify({id:current,method,params}));
  });
}
socket.addEventListener('message',({data})=>{
  const value=JSON.parse(data), request=pending.get(value.id);
  if(request) { clearTimeout(request.timer);pending.delete(value.id); value.error?request.reject(new Error(value.error.message)):request.resolve(value.result); }
  if(value.method==='Runtime.exceptionThrown' || value.method==='Log.entryAdded' && value.params.entry.level==='error'
      || value.method==='Runtime.consoleAPICalled' && value.params.type==='error') errors.push(value);
});
socket.addEventListener('close',()=>{for(const request of pending.values()){clearTimeout(request.timer);request.reject(new Error('Browser disconnected'));}pending.clear();});
try {
  await new Promise((resolve,reject)=>{
    const timer=setTimeout(()=>reject(new Error('CDP connection timed out')),5000);
    socket.addEventListener('open',()=>{clearTimeout(timer);resolve();},{once:true});
    socket.addEventListener('error',(error)=>{clearTimeout(timer);reject(error);},{once:true});
  });
  await command('Runtime.enable'); await command('Log.enable'); await command('Page.enable');
  await command('Page.navigate',{url:viewer.href});
  const deadline=Date.now()+15000;
  for(;;) {
    const state=await command('Runtime.evaluate',{expression:`location.href===${JSON.stringify(viewer.href)} && document.readyState==='complete'`,returnByValue:true}).catch(()=>null);
    if(state?.result?.value===true) break;
    if(Date.now()>deadline) throw new Error('Viewer document did not load');
    await new Promise((resolve)=>setTimeout(resolve,50));
  }
  const result=await command('Runtime.evaluate',{expression:`(${checkNavigation.toString()})()`,awaitPromise:true,returnByValue:true});
  if(result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description??result.exceptionDetails.text);
  assert.equal(errors.length,0,JSON.stringify(errors));
  console.log(JSON.stringify(result.result.value,null,2));
} finally {
  socket.close();
  await fetch(new URL(`/json/close/${target.id}`,endpoint),{signal:AbortSignal.timeout(5000)}).catch(()=>{});
}

async function checkNavigation() {
  const $=(id)=>document.getElementById(id);
  const check=(condition,message)=>{if(!condition)throw new Error(message);};
  const wait=async(test)=>{
    const deadline=performance.now()+30000;
    while(!test()){if(performance.now()>deadline)throw new Error(`Viewer timeout: ${$('status')?.textContent}; ${$('error')?.textContent}`);await new Promise((resolve)=>setTimeout(resolve,25));}
  };
  const frames=()=>new Promise((resolve)=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
  await wait(()=>$('status')?.textContent==='Ready — open a PDF');
  check(crossOriginIsolated,'Viewer must be cross-origin isolated');
  const {navigationDocument}=await import(new URL('./navigation_document.mjs',location.href));
  const data=new DataTransfer(); data.items.add(new File([navigationDocument()],'content-fits.pdf',{type:'application/pdf'}));
  $('file').files=data.files; $('file').dispatchEvent(new Event('change',{bubbles:true}));
  const ready=()=>$('status').textContent==='Ready' && $('error').hidden;
  await wait(()=>ready() && document.querySelectorAll('[role=treeitem]').length===6);
  $('outline-tab').click();
  const samples=[];
  for(let index=0;index<6;index++) {
    const viewport=$('viewport'), painted=index<3, page=painted?1:2;
    const width=painted?120:600,height=painted?200:800;
    const availableWidth=viewport.clientWidth-48,availableHeight=viewport.clientHeight-48;
    const raw=index%3===0?Math.min(availableWidth/width,availableHeight/height):index%3===1?availableWidth/width:availableHeight/height;
    const zoom=Math.max(0.1,Math.min(8,raw)), title=document.querySelectorAll('[role=treeitem]')[index].textContent;
    document.querySelectorAll('[role=treeitem]')[index].click();
    await wait(()=>ready() && $('page').value===String(page+1) && Math.abs(parseFloat(document.querySelector(`[data-page="${page}"]`)?.style.width)-600*zoom)<1e-5);
    await frames();
    check($('page').value===String(page+1),`Page field changed after ${title}`);
    check($('page-label').textContent===`Label: ${painted?'Painted':'Empty'}`,`Wrong label after ${title}`);
    if(painted) {
      const area=viewport.getBoundingClientRect(), sheet=document.querySelector(`[data-page="${page}"]`).getBoundingClientRect();
      const left=sheet.left+60*zoom-area.left, top=sheet.top+500*zoom-area.top;
      if(index===0) {
        check(left>=23 && left+120*zoom<=viewport.clientWidth-23,`${title}: painted width is not visible`);
        check(top>=23 && top+200*zoom<=viewport.clientHeight-23,`${title}: painted height is not visible`);
      } else check(Math.abs(left-24)<=1 && Math.abs(top-24)<=1,`${title}: destination coordinates were not honoured (${left}, ${top})`);
    }
    samples.push({destination:title,page:page+1,zoom,scrollTop:viewport.scrollTop});
  }
  $('zoom').value='1'; $('zoom').dispatchEvent(new Event('change'));
  $('page').value='#2'; $('page').dispatchEvent(new Event('change')); $('page').blur();
  await wait(()=>ready() && $('page').value==='2' && document.querySelector('[data-page="1"]')?.style.width==='600px');
  const viewport=$('viewport'), page=document.querySelector('[data-page="1"]');
  viewport.scrollTop+=page.getBoundingClientRect().top-viewport.getBoundingClientRect().top-32;
  await frames(); await wait(()=>ready() && $('page').value==='2');
  check($('page').value==='2','Previous-page strip must not replace the destination page');
  samples.push({navigation:'previous-page strip',page:2,scrollTop:viewport.scrollTop});
  return {userAgent:navigator.userAgent,crossOriginIsolated,samples};
}
