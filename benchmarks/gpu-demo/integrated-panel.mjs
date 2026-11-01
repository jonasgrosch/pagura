import {BrowserPage} from './integrated.mjs';
import {stats} from './measure.mjs';
export function installIntegratedPanel(demo) {
  const $=id=>document.getElementById(id),modes=['cpu','retained','world'],names=['CPU prepared reuse','WebGPU per DPI','WebGPU world geometry'];let pages=[],pending=null;
  async function clear(){for(const page of pages)await page.destroy();pages=[];$('integrated-scroll').disabled=true;}
  $('integrated-run').addEventListener('click',async()=>{
    if(demo.busy||!demo.input||!demo.gpu)return;demo.busy=true;$('integrated-run').disabled=true;
    $('integrated-status').textContent='Opening page 1 in fresh workers, then measuring retained scrolling…';
    try {
      await clear();const rows=[];
      for(let i=0;i<modes.length;i++){
        const started=performance.now(),page=await BrowserPage.create(modes[i],demo.gpu,demo.input,$(`integrated-${modes[i]}`));pages.push(page);
        await page.render(150);const first=performance.now()-started,samples=[];
        const before=page.counters();for(let n=-2;n<9;n++){const out=await page.scroll((n+2)/10);if(n>=0)samples.push(out.frame_ms);}
        if(JSON.stringify(before)!==JSON.stringify(page.counters()))throw new Error('Scroll rendered new page content');
        const accuracy=await page.accuracy(150);rows.push({name:names[i],first,scroll:stats(samples),accuracy,fallback:page.fallback});
      }
      demo.lastIntegrated=rows;$('integrated-results').replaceChildren();
      for(const row of rows){const tr=document.createElement('tr');for(const value of [row.name,row.first.toFixed(2)+' ms',row.scroll.median.toFixed(2)+' / '+row.scroll.p95.toFixed(2)+' ms',row.fallback??`${row.accuracy.difference.max_channel_error} levels`]){const td=document.createElement('td');td.textContent=value;tr.append(td);}$('integrated-results').append(tr);}
      $('integrated-scroll').disabled=false;$('integrated-status').textContent='Page 1 · 150 DPI. First display includes a fresh worker and document open; adapter and application stay warm. Scrolling reuses each full-page bitmap with zero engine calls or coverage dispatches. Nine scroll samples after two warmups; two animation-frame opportunities, not scanout.';
    }catch(error){$('integrated-status').textContent=String(error);}
    finally{demo.busy=false;$('integrated-run').disabled=false;}
  });
  $('integrated-scroll').addEventListener('input',async()=>{
    pending=Number($('integrated-scroll').value)/100;if(demo.busy)return;demo.busy=true;
    try{while(pending!==null){const fraction=pending;pending=null;for(const page of pages)await page.scroll(fraction);}}catch(error){$('integrated-status').textContent=String(error);}finally{demo.busy=false;}
  });
  return {clear,enable:()=>{$('integrated-run').disabled=!demo.gpu;}};
}
