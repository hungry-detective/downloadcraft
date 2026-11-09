const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const rank = require('./rank.js');
const source = fs.readFileSync(__dirname + '/background.js', 'utf8');
// pauseFails: the browser cannot pause it (finished, or never started).
function harness(accept=true, cancelFails=false, cfg={autoCapture:true,minSizeKB:0}, pauseFails=cancelFails, ctx={}) {
  const calls=[], handlers={};
  const skips=new Set();
  const context={ console, Map, Object, Promise,
    settings:async()=>cfg, hostBlocked:()=>false, freshSeen:()=>false,
    skipped:(u)=>skips.has(u), typeAllowed:(c,n,u)=>!c.captureTypes || c.captureTypes.split(' ').some(t=>String(n||u).endsWith('.'+t)),
    DCBridge:{browserContext:()=>ctx},
    sendToHost:async(...args)=>{calls.push(['host',...args]);return accept;},
    syncBrowserDownloadUI:async()=>{},setBadge:()=>{},markSeen:(...a)=>calls.push(['seen',...a]),
    hostOk:false,lastFallbackError:'',
    api:{downloads:{cancel:async(id)=>{calls.push(['cancel',id]);if(cancelFails)throw Error('complete');},erase:async(x)=>calls.push(['erase',x]),
      pause:async(id)=>{calls.push(['pause',id]);if(pauseFails)throw Error('cannot pause');},resume:async(id)=>calls.push(['resume',id])}},
    chrome:{downloads:{download:(x,cb)=>{calls.push(['fallback',x]);cb();},
      onCreated:{addListener:f=>handlers.created=f},onDeterminingFilename:{addListener:f=>handlers.filename=f}},runtime:{}}
  };
  vm.createContext(context);
  vm.runInContext(source.slice(source.indexOf('async function performBrowserCapture('),source.indexOf('// ---- popup / content-script messages ----')),context);
  return {calls,handlers,context,skips};
}
const item={id:5,url:'https://example.com/file.zip',filename:'file.zip',referrer:'https://example.com/page'};
test('Brave creation and filename events share one cancellation and native handoff',async()=>{
  const h=harness();let released=0;
  h.handlers.created(item);
  assert.equal(h.handlers.filename({...item,filename:'resolved.zip'},()=>released++),true);
  await vm.runInContext('captureBrowserDownload({id:5})',h.context);
  assert.equal(h.calls.filter(x=>x[0]==='cancel').length,1);
  assert.equal(h.calls.filter(x=>x[0]==='host').length,1);
  assert.equal(h.calls.find(x=>x[0]==='host')[2],'resolved.zip');
  assert.equal(h.calls.filter(x=>x[0]==='erase').length,1);
  assert.equal(released,1);
});
test('missing host releases filename callback and resumes the paused browser download',async()=>{
  const h=harness(false);let released=0;
  h.handlers.filename(item,()=>released++);
  await vm.runInContext('captureBrowserDownload({id:5})',h.context);
  assert.equal(released,1);
  // Paused, not cancelled: the browser simply carries on - no second request.
  assert.equal(h.calls.filter(x=>x[0]==='pause').length,1);
  assert.equal(h.calls.filter(x=>x[0]==='resume').length,1);
  assert.equal(h.calls.filter(x=>x[0]==='cancel').length,0);
  assert.equal(h.calls.filter(x=>x[0]==='fallback').length,0);
  assert.equal(h.calls.filter(x=>x[0]==='erase').length,0);
});
test('a download that cannot pause is cancelled, and re-downloaded when the app is missing',async()=>{
  const h=harness(false,false,{autoCapture:true,minSizeKB:0},true);
  await vm.runInContext('captureBrowserDownload('+JSON.stringify(item)+')',h.context);
  assert.equal(h.calls.filter(x=>x[0]==='cancel').length,1);
  assert.equal(h.calls.filter(x=>x[0]==='fallback').length,1);
});
test('the app accepting a paused download cancels and erases the browser copy',async()=>{
  const h=harness(true);
  await vm.runInContext('captureBrowserDownload('+JSON.stringify(item)+')',h.context);
  const order=h.calls.map(x=>x[0]).filter(x=>['pause','host','cancel','erase'].includes(x));
  assert.deepEqual(order,['pause','host','cancel','erase']);
});
test('the keep-in-browser key and the file-type list leave downloads alone',async()=>{
  const h=harness();
  h.skips.add(item.url);
  await vm.runInContext('captureBrowserDownload('+JSON.stringify(item)+')',h.context);
  assert.equal(h.calls.length,0);
  const t=harness(true,false,{autoCapture:true,minSizeKB:0,captureTypes:'iso exe'});
  await vm.runInContext('captureBrowserDownload('+JSON.stringify(item)+')',t.context);
  assert.equal(t.calls.length,0);
});
test('completed browser download is not duplicated',async()=>{
  const h=harness(true,true);
  await vm.runInContext('captureBrowserDownload('+JSON.stringify(item)+')',h.context);
  assert.equal(h.calls.filter(x=>x[0]==='host').length,0);
});
test('capture disabled or below minimum size leaves Brave in control',async()=>{
  for(const cfg of [{autoCapture:false},{autoCapture:true,minSizeKB:100}]){
    const h=harness(true,false,cfg);
    await vm.runInContext('captureBrowserDownload('+JSON.stringify({...item,totalBytes:100})+')',h.context);
    assert.equal(h.calls.length,0);
  }
});
test('direct handoff accepts installers/download anchors and preserves normal navigation',()=>{
  for(const url of ['https://h/a.ZIP?token=x','https://h/a.appimage','https://h/a.exe'])assert.ok(rank.directDownloadLink(url,false));
  for(const url of ['magnet:?xt=urn:btih:abc&dn=x','https://h/linux.iso.torrent'])assert.ok(rank.directDownloadLink(url,false));
  assert.equal(rank.directDownloadLink('magnet:',false),false);
  assert.ok(rank.directDownloadLink('https://h/opaque?id=1',true));
  for(const url of ['https://h/watch.mp4','https://h/doc.pdf','https://h/page','blob:https://h/a','javascript:alert(1)'])assert.equal(rank.directDownloadLink(url,false),false);
});
test('dismissal follows a video while allowing another video and resets by page reload',()=>{
  assert.equal(rank.videoDismissKey('https://h/watch#x',null,'a'),rank.videoDismissKey('https://h/watch#y',null,'a'));
  assert.notEqual(rank.videoDismissKey('https://h/watch',null,'a'),rank.videoDismissKey('https://h/watch',null,'b'));
  assert.notEqual(rank.videoDismissKey('https://h/watch','blob:a'),rank.videoDismissKey('https://h/watch','blob:b'));
});

test('fallback suppression survives Brave resolving a different filename',()=>{
  const context={seen:new Map(),GRACE_MS:10000,Date};vm.createContext(context);
  vm.runInContext(source.slice(source.indexOf('function markSeen('),source.indexOf('async function settings(')),context);
  vm.runInContext("markSeen('https://h/file',null)",context);
  assert.equal(vm.runInContext("freshSeen('https://h/file','/Downloads/resolved-name.zip')",context),true);
  assert.equal(vm.runInContext("freshSeen('https://h/another','/Downloads/other.zip')",context),false);
});

test('video X removes the actual overlay/menu and remembers the dismissed video',()=>{
  const content=fs.readFileSync(__dirname+'/content.js','utf8');
  const controls=[];
  const element=()=>({isConnected:true,style:{},setAttribute(){},remove(){this.isConnected=false;},getBoundingClientRect(){return {width:32,height:32,right:80,top:30};}});
  const button=element(),panel=element();button.__dcClose=element();
  const context={Set,DCRank:rank,location:{href:'https://h/watch'},button,panel,
    document:{querySelectorAll:()=>controls,createElement:element,
      documentElement:{appendChild(e){e.parentNode=this;if(!controls.includes(e))controls.push(e);}},addEventListener(){}},window:{addEventListener(){}}};
  vm.createContext(context);
  vm.runInContext(content.slice(content.indexOf('  const dismissedVideos ='),content.indexOf('  function ensureButton(')),context);
  vm.runInContext("addDismissControl(button,'video-a',panel)",context);
  assert.equal(button.__dcClose.hidden,false);
  let stopped=false;
  button.__dcClose.onclick({preventDefault(){},stopPropagation(){stopped=true;}});
  assert.equal(button.isConnected,false);assert.equal(panel.isConnected,false);
  assert.ok(stopped);
  assert.equal(vm.runInContext("dismissedVideos.has('video-a')",context),true);
  assert.equal(vm.runInContext("dismissedVideos.has('video-b')",context),false);
});


test('redirect resolved during settings read is used for the handoff',async()=>{
  const h=harness();
  h.handlers.created(item);
  const redirected={...item,finalUrl:'https://cdn.example.com/real.zip'};
  h.handlers.filename(redirected,()=>{});
  await vm.runInContext('captureBrowserDownload({id:5})',h.context);
  assert.equal(h.calls.find(x=>x[0]==='host')[1],redirected.finalUrl);
});


test('POST downloads without a known body stay in the browser',async()=>{
  const h=harness();
  vm.runInContext('rememberDownloadRequest({url:"https://example.com/file.zip",method:"POST"})',h.context);
  await vm.runInContext('captureBrowserDownload('+JSON.stringify(item)+')',h.context);
  assert.equal(h.calls.length,0);
});
test('POST downloads with their body go to the app',async()=>{
  const h=harness(true,false,{autoCapture:true,minSizeKB:0},false,{_post:true,method:'POST',body:'YT0x'});
  vm.runInContext('rememberDownloadRequest({url:"https://example.com/file.zip",method:"POST"})',h.context);
  await vm.runInContext('captureBrowserDownload('+JSON.stringify(item)+')',h.context);
  assert.equal(h.calls.filter(x=>x[0]==='host').length,1);
});
test('GET requests remain eligible for automatic takeover',async()=>{
  const h=harness();
  vm.runInContext('rememberDownloadRequest({url:"https://example.com/file.zip",method:"GET"})',h.context);
  await vm.runInContext('captureBrowserDownload('+JSON.stringify(item)+')',h.context);
  assert.equal(h.calls.filter(x=>x[0]==='host').length,1);
});
