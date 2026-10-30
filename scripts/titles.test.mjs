import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile, mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { fileURLToPath } from 'node:url';
import { request } from 'node:http';
import { cleanTitle, taskTitle, paneTitle, tabTitle, titleSnapshot } from '../defaults/mods/tabs/titles.js';
import { tools, callTool, rpc, httpServer } from '../defaults/mods/tabs/agent.mjs';
const agent = { pane:'p1', tab:'remote/tb1', terminal:'remote/t1', title:'⠋ Fix login', cwd:'/srv/project', program:{name:'Codex', key:'codex', agent:true} };
const tab = {id:'remote/tb1', name:'1', named:false};

test('clean task titles preserve Unicode and meaningful punctuation', () => {
  for (const prefix of ['⠋ ', '⠙ ', '✳ ', '✻ ']) assert.equal(cleanTitle(prefix + '修复 🙂 #42'), '修复 🙂 #42');
  assert.equal(cleanTitle('\x1b[31mFix\nlogin\x1b[0m'), 'Fix login');
  assert.equal(cleanTitle('#42: fix .env'), '#42: fix .env');
  assert.equal(cleanTitle(''), '');
});
test('manual titles always win, including program names and numbers', () => {
  for (const name of ['Codex','1','tb123','My task']) assert.equal(tabTitle({...tab,name,named:true},[agent]),name);
  assert.equal(tabTitle({...tab,named:undefined,name:'Legacy name'},[agent]),'Legacy name');
});
test('automatic titles follow live work and stay stable across split focus', () => {
  const shell = {...agent, pane:'p2', program:{name:'Bash',shell:true}, title:'/srv/project', focused:true};
  assert.equal(tabTitle(tab,[shell,agent]),'Fix login');
  assert.equal(tabTitle(tab,[{...shell,focused:false},{...agent,focused:true}]),'Fix login');
  assert.equal(tabTitle(tab,[{...agent,title:'Add search'}]),'Add search');
  assert.equal(tabTitle(tab,[{...agent,tab:'other/tb1'},shell]),'Bash · project');
  assert.equal(tabTitle(tab,[]),'1');
});
test('fallbacks reject transient status, generic names and prompts', () => {
  for (const title of ['Codex','CODEX','⠋ Codex','Thinking…','working...','/srv/project','~/project','C:\\work','user@host:/work','']) {
    assert.equal(taskTitle({...agent,title}),'',title);
    assert.equal(paneTitle({...agent,title}),'Codex · project',title);
  }
  assert.equal(paneTitle({...agent,title:'',cwd:'C:\\work\\repo\\'}),'Codex · repo');
  assert.equal(paneTitle({mod:'files',title:'Files'}),'Files');
  assert.equal(taskTitle({...agent,title:'Fix @mentions'}),'Fix @mentions');
});
test('every independent mod has the same policy, including clones', async () => {
  const canonical = await readFile(new URL('../defaults/mods/tabs/titles.js',import.meta.url),'utf8');
  for (const mod of ['agents','pane','peek','picker','terminals-bar','terminals-pane','win98'])
    assert.equal(await readFile(new URL(`../defaults/mods/${mod}/titles.js`,import.meta.url),'utf8'),canonical,mod);
});
test('snapshots expose title modes, per-pane tasks and terminal names', () => {
  const snapshot = titleSnapshot([{id:'s1',name:'project',named:false,tabs:[tab]}],[agent],[{...agent,address:'remote/t1',name:'Pinned worker'}]);
  assert.equal(snapshot.spaces[0].tabs[0].title,'Fix login');
  assert.equal(snapshot.spaces[0].tabs[0].mode,'automatic');
  assert.equal(snapshot.panes[0].title,'Fix login');
  assert.equal(snapshot.terminals[0].title,'Pinned worker');
});
test('all title UI actions have both API and MCP coverage', async () => {
  const files=['tabs/main.js','menu/main.js','new-menu/main.js','win98/taskbar.js'];
  const commands = new Set(tools.map(t=>t.command).filter(Boolean));
  for (const file of files) {
    const source=await readFile(new URL(`../defaults/mods/${file}`,import.meta.url),'utf8');
    for (const match of source.matchAll(/"(tab\.(?:rename|title[\w.]*|auto[\w.]*))"/g)) assert.ok(commands.has(match[1]),`${file}: uncovered ${match[1]}`);
  }
  for (const tool of tools) {
    assert.ok(tool.http.method && tool.http.path && tool.inputSchema);
    assert.ok(tool.uiActions.length);
  }
  assert.ok(tools.some(t=>t.uiActions.includes('Use automatic title')));
  for (const file of ['new-menu/main.js','win98/taskbar.js']) {
    const source=await readFile(new URL(`../defaults/mods/${file}`,import.meta.url),'utf8');
    assert.doesNotMatch(source,/tab\.new["'][^\n]*\{ name(?:,|: (?:agent|a)\.name)/);
  }
});
test('writes validate arguments and pass remote IDs literally without a shell', async () => {
  const calls=[]; const run=async args=>{calls.push(args);return {ok:true};};
  const result=await callTool('herdr_tab_title_set',{id:'remote/tb1',name:'$(touch /never); "hello"'},run);
  assert.deepEqual(calls,[['do','tab.rename',JSON.stringify({id:'remote/tb1',name:'$(touch /never); "hello"'})]]);
  assert.equal(result.mode,'manual');
  assert.equal((await callTool('herdr_tab_title_set',{id:'remote/tb1',name:''},run)).mode,'automatic');
  for (const args of [{id:'x'}, {id:'',name:''}, {id:'x',name:1}, {id:'x',name:'',extra:true}, null])
    await assert.rejects(callTool('herdr_tab_title_set',args,run));
  assert.equal(calls.length,2);
});
test('inspection returns only title projection, refuses missing/stale snapshots', async () => {
  const dir=await mkdtemp(join(tmpdir(),'herdr-title-test-')); const prior=process.env.HERDR_APP_HOME;
  process.env.HERDR_APP_HOME=dir;
  try {
    const data=titleSnapshot([{id:'s1',name:'project',tabs:[tab]}],[agent],[]);
    await writeFile(join(dir,'shared.json'),JSON.stringify({'titles.snapshot':{updated_at:'2026-01-01',data},unrelated:'private'}));
    const result=await callTool('herdr_titles_list',{},async()=>({spaces:data.spaces}));
    assert.equal(result.source,'last-rendered'); assert.ok(!JSON.stringify(result).includes('private'));
    await assert.rejects(callTool('herdr_titles_list',{},async()=>({spaces:[]})),/catching up/);
    await assert.rejects(callTool('herdr_titles_list',{},async()=>{throw Error('offline');}),/offline/);
  } finally { if(prior===undefined) delete process.env.HERDR_APP_HOME; else process.env.HERDR_APP_HOME=prior; await rm(dir,{recursive:true}); }
});
test('MCP discovery, structured results, errors and notifications', async () => {
  const request=(method,params)=>({jsonrpc:'2.0',id:1,method,params});
  assert.equal((await rpc(request('initialize',{}))).result.protocolVersion,'2025-11-25');
  assert.equal((await rpc(request('tools/list'))).result.tools.length,2);
  assert.deepEqual((await rpc(request('tools/call',{name:'test'}),async()=>({ok:true}))).result.structuredContent,{ok:true});
  assert.equal((await rpc(request('tools/call',{name:'unknown'}))).result.isError,true);
  assert.equal(await rpc({jsonrpc:'2.0',method:'notifications/initialized'}),null);
  assert.equal((await rpc(request('missing'))).error.code,-32601);
});
test('stdio adapter works as a real MCP subprocess', async () => {
  const child=spawn(process.execPath,[fileURLToPath(new URL('../defaults/mods/tabs/agent.mjs',import.meta.url))],{stdio:['pipe','pipe','pipe']});
  let output=''; child.stdout.on('data',x=>output+=x);
  child.stdin.end(JSON.stringify({jsonrpc:'2.0',id:7,method:'tools/list'})+'\n');
  const [code]=await once(child,'exit'); assert.equal(code,0);
  assert.equal(JSON.parse(output).id,7);
});
test('HTTP provides equivalent tools and rejects browser cross-origin writes', async () => {
  const calls=[]; const server=httpServer(async(name,args)=>{calls.push({name,args});return {ok:true};});
  server.listen(0,'127.0.0.1'); await once(server,'listening');
  const url=`http://127.0.0.1:${server.address().port}`;
  try {
    assert.equal((await (await fetch(url+'/v1/tools')).json()).tools.length,2);
    assert.equal((await fetch(url+'/v1/tab-title',{method:'POST',body:JSON.stringify({id:'remote/tb1',name:''})})).status,200);
    assert.deepEqual(calls,[{name:'herdr_tab_title_set',args:{id:'remote/tb1',name:''}}]);
    assert.equal((await fetch(url+'/v1/tab-title',{method:'POST',headers:{Origin:'https://example.com'},body:'{}'})).status,403);
    const status = await new Promise(resolve => { const req=request(url+'/v1/titles',{headers:{Host:'evil.example'}},res=>{res.resume();resolve(res.statusCode);}); req.end(); });
    assert.equal(status,403);
    assert.equal(calls.length,1);
  } finally { server.closeAllConnections(); await new Promise(resolve=>server.close(resolve)); }
});
