import assert from 'node:assert/strict';
import {after, test} from 'node:test';
import {mkdtemp, rm} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {build} from 'esbuild';

// Exercise the real Studio facade with a connection fixture and no Extension Host process.
const extensionRoot = fileURLToPath(new URL('../', import.meta.url));
const repositoryRoot = fileURLToPath(new URL('../../../', import.meta.url));
const scratch = await mkdtemp(join(tmpdir(), 'runtrol-routing-test-'));
let cleaned = false;
const cleanup = async () => {
  if (cleaned) return;
  await rm(scratch, {recursive: true});
  cleaned = true;
};
after(cleanup);
const bundle = join(scratch, 'facade.cjs');
let api;
try {
  await build({
    stdin: {contents: [
      'export {StudioRuntimeClient} from "./src/runtimeClient";',
      'export {RuntimeLocator,IntegrationIdentity,IntegrationCredentials} from "@runtrol/runtime-client";',
      'export {validatedLocator} from "../../clients/typescript/src/testing";',
      'export {STUDIO_SCOPES} from "./src/studioInputPriority";',
    ].join('\n'), resolveDir: extensionRoot, loader: 'ts'},
    outfile: bundle, bundle: true, platform: 'node', format: 'cjs', target: 'node20',
    alias: {'@runtrol/runtime-client': join(repositoryRoot, 'clients/typescript/src/index.ts')},
    plugins: [{name: 'workspace-fixture', setup(builder) {
      builder.onResolve({filter: /^vscode$/}, () => ({path: 'vscode', namespace: 'fixture'}));
      builder.onLoad({filter: /.*/, namespace: 'fixture'}, () => ({
        contents: 'export const workspace={workspaceFolders:[]};', loader: 'js',
      }));
    }}],
  });
  api = createRequire(import.meta.url)(bundle);
} catch (error) {
  await cleanup();
  throw error;
}
const {StudioRuntimeClient, RuntimeLocator, IntegrationIdentity, IntegrationCredentials, validatedLocator, STUDIO_SCOPES} = api;
const deferred = () => { let resolve; const promise = new Promise(done => {resolve = done;}); return {promise, resolve}; };
async function until(check) {
  for (let n = 0; n < 200; n++) { if (check()) return; await new Promise(resolve => setImmediate(resolve)); }
  throw new Error('bounded facade deadline');
}
function stream() {
  const pending = [];
  let ended = false;
  return {
    next() { if (ended) return Promise.resolve({kind:'ended',ended:{reason:'runtimeUnavailable'}}); const wait = deferred(); pending.push(wait); return wait.promise; },
    close() { ended = true; for (const wait of pending.splice(0)) wait.resolve({kind:'ended',ended:{reason:'runtimeUnavailable'}}); },
  };
}
function fixture(openGate) {
  const a = validatedLocator('home', 'pipe-a', '0.1.1', 'a'.repeat(64));
  const b = validatedLocator('home', 'pipe-b', '0.1.1', 'b'.repeat(64));
  let primary = a, publish, watchStopped = false;
  const snapshot = () => ({current:{state:'running',locator:primary},currentRevision:primary.revision,generations:[a,b]});
  const original = RuntimeLocator.system;
  RuntimeLocator.system = () => ({
    watchGenerations(receive,{signal}) {
      publish = receive;
      receive(snapshot());
      return new Promise(resolve => signal.addEventListener('abort',()=>{watchStopped=true;resolve();},{once:true}));
    },
  });
  const grant = {integrationId:'int_test',scopes:STUDIO_SCOPES,roots:[],keyGeneration:1,grantGeneration:1};
  const identity = IntegrationIdentity.generate();
  const secrets = new Map();
  const context = {extension:{packageJSON:{version:'0.1.1'}},secrets:{async get(key){return secrets.get(key);},async store(key,value){secrets.set(key,value);}}};
  const client = new StudioRuntimeClient(context,async()=>({runtimeExecutable:'unused',preferDigest:null}),async()=>({runtimeExecutable:'unused',preferDigest:null}),async()=>true,async()=>false,async()=>false,async()=>false);
  client.stored = {schema:1,clientInstanceId:'test',privateKeyPkcs8:Buffer.from(identity.exportPkcs8()).toString('base64url'),grant,inputPriorityReviewed:true};
  client.options = {name:'Owned facade verification',version:'0.0.0',credentials:new IntegrationCredentials(identity,grant)};
  const connections = [], output = [], ended = [], opened = [];
  let registrationGeneration = 0;
  client.connector.connectWithRetry = async locator => {
    const subscriptions=[];
    const connection = {
      locator, closed:0,
      initialization:{grant,runtime:{instanceId:'home'},serverCapabilities:{terminalInputPriority:true}},
      close(){this.closed++;for(const subscription of subscriptions)subscription.close();},
      windows(){return {
        register:async()=>({windowSessionId:'window',registrationGeneration:++registrationGeneration,ownerToken:'owner-proof'}),
        update:async()=>undefined,
        watchReveals:async()=>{const subscription=stream();subscriptions.push(subscription);return subscription;},
        watchInput:async()=>{const subscription=stream();subscriptions.push(subscription);return subscription;},
        mirrorOpen:async()=>({terminalId:'mirror-'+locator.endpoint}),
        mirrorOutput:async params=>{output.push({locator,params});},
        mirrorEnd:async params=>{ended.push({locator,params});},
        reveal:async()=>({delivered:true,foreground:'focused'}),
      };},
      terminals(){return {open:async params=>{
        opened.push({locator,params});
        if(openGate)await openGate.promise;
        return {locator,close(){connection.close();}};
      }};},
    };
    connections.push(connection);
    return connection;
  };
  const state = {register:{windowSessionId:'window',hostGeneration:'test-host',vscodeVersion:'1.0',hostPid:1,workspaceFolders:[]},update:{terminals:[]}};
  const owner = {state:()=>state,reveal:()=>true,input:async(_group,subscription)=>{while((await subscription.next()).kind!=='ended'){}},failed:()=>undefined};
  return {client,a,b,state,owner,connections,output,ended,opened,secrets,context,
    move(){primary=b;publish(snapshot());},
    get watchStopped(){return watchStopped;},
    close(){client.dispose();RuntimeLocator.system=original;},
  };
}

test('Studio fresh opens select the successor while an admitted dedicated view keeps its original route',async()=>{
  const gate=deferred(),f=fixture(gate);
  try{
    const first=f.client.openTerminal({providerId:'test',target:{kind:'fresh'},workspace:'workspace'});
    await until(()=>f.opened.length===1);
    f.move();
    const second=f.client.openTerminal({providerId:'test',target:{kind:'fresh'},workspace:'workspace'});
    await until(()=>f.opened.length===2);
    assert.equal(f.opened[0].locator.revision,f.a.revision);
    assert.equal(f.opened[1].locator.revision,f.b.revision);
    assert.equal(f.connections.filter(connection=>connection.locator===f.a).some(connection=>connection.closed===0),true);
    gate.resolve();
    const views=await Promise.all([first,second]);
    assert.equal(views[0].locator.revision,f.a.revision);
    for(const view of views)view.close();
  }finally{gate.resolve();f.close();}
  assert.equal(f.watchStopped,true);
});

test('exact owner reveal joins the initial route registration before its commit',async()=>{
  const f=fixture(), gate=deferred();
  let registrations=0, revealSettled=false;
  const connect=f.client.connector.connectWithRetry;
  f.client.connector.connectWithRetry=async(...args)=>{
    const connection=await connect(...args), windows=connection.windows();
    connection.windows=()=>({...windows,register:async params=>{
      registrations++;
      await gate.promise;
      return windows.register(params);
    }});
    return connection;
  };
  try{
    f.client.setWindowOwner(f.owner);
    const initial=f.client.commandClient();
    await until(()=>registrations===1);
    const reveal=f.client.revealAtOwner({windowSessionId:'other',terminalKey:'shell'},f.a.digest)
      .then(value=>{revealSettled=true;return value;});
    await new Promise(resolve=>setImmediate(resolve));
    assert.equal(revealSettled,false,'a joining reveal waits for the existing registration');
    gate.resolve();
    await initial;
    assert.equal((await reveal).delivered,true);
    assert.equal(registrations,1);
  }finally{gate.resolve();f.close();}
});

test('late activity connection is discarded after a primary switch and the observation runs once on the successor',async()=>{
  const f=fixture(), gate=deferred();
  let waiting=false, oldConnection, actions=0;
  try{
    await f.client.commandClient();
    const connect=f.client.connector.connectWithRetry;
    f.client.connector.connectWithRetry=async(...args)=>{
      const connection=await connect(...args);
      if(!waiting){waiting=true;oldConnection=connection;await gate.promise;}
      return connection;
    };
    const observed=f.client.activityRead(async runtime=>{actions++;return runtime.locator;});
    await until(()=>waiting);
    f.move();
    await until(()=>f.client.command?.locator===f.b);
    gate.resolve();
    assert.equal((await observed).revision,f.b.revision);
    assert.equal(oldConnection.closed,1);
    assert.equal(actions,1);
    assert.equal(f.client.activity.locator.revision,f.b.revision);
  }finally{gate.resolve();f.close();}
});

test('a disposed Studio closes a late activity connection without publishing it',async()=>{
  const f=fixture(), gate=deferred();
  let waiting=false, late;
  try{
    await f.client.commandClient();
    const connect=f.client.connector.connectWithRetry;
    f.client.connector.connectWithRetry=async(...args)=>{late=await connect(...args);waiting=true;await gate.promise;return late;};
    const ended=assert.rejects(f.client.activityRead(async()=>assert.fail('disposed observation')));
    await until(()=>waiting);
    f.close();
    gate.resolve();
    await ended;
    assert.equal(late.closed,1);
    assert.equal(f.client.activity,null);
  }finally{gate.resolve();f.close();}
});

test('a previous identity connection completing after reauthentication cannot replace credentials or stored identity',async()=>{
  const f=fixture(), gate=deferred();
  let waiting=false, late;
  try{
    const connect=f.client.connector.connectWithRetry;
    f.client.connector.connectWithRetry=async(...args)=>{
      late=await connect(...args);
      late.initialization.grant={...late.initialization.grant,grantGeneration:2};
      waiting=true;await gate.promise;return late;
    };
    const ended=assert.rejects(f.client.connectCommand(f.a,new AbortController().signal),/integration changed/);
    await until(()=>waiting);
    const identity=IntegrationIdentity.generate();
    const nextGrant={...f.client.options.credentials.grant,integrationId:'int_reauthenticated'};
    const nextOptions={...f.client.options,credentials:new IntegrationCredentials(identity,nextGrant)};
    const nextStored={...f.client.stored,grant:nextGrant,privateKeyPkcs8:Buffer.from(identity.exportPkcs8()).toString('base64url')};
    f.client.integrationEpoch++;
    f.client.options=nextOptions;
    f.client.stored=nextStored;
    gate.resolve();
    await ended;
    assert.equal(f.client.options,nextOptions);
    assert.equal(f.client.stored,nextStored);
    assert.equal(f.secrets.size,0);
    assert.equal(late.closed,1);
  }finally{gate.resolve();f.close();}
});

test('a grant save already in flight cannot overwrite the newer identity after reauthentication',async()=>{
  const f=fixture(), gate=deferred();
  let saving=false;
  try{
    const connect=f.client.connector.connectWithRetry;
    f.client.connector.connectWithRetry=async(...args)=>{const connection=await connect(...args);connection.initialization.grant={...connection.initialization.grant,grantGeneration:2};return connection;};
    f.context.secrets.store=async(key,value)=>{if(!saving){saving=true;await gate.promise;}f.secrets.set(key,value);};
    const ended=assert.rejects(f.client.connectCommand(f.a,new AbortController().signal),/integration changed/);
    await until(()=>saving);
    const identity=IntegrationIdentity.generate();
    const grant={...f.client.options.credentials.grant,integrationId:'int_new_identity'};
    const options={...f.client.options,credentials:new IntegrationCredentials(identity,grant)};
    const stored={...f.client.stored,grant,privateKeyPkcs8:Buffer.from(identity.exportPkcs8()).toString('base64url')};
    f.client.integrationEpoch++;
    f.client.options=options;
    const saved=f.client.persistIntegration(stored);
    gate.resolve();
    await Promise.all([ended,saved]);
    assert.equal(f.client.options,options);
    assert.equal(f.client.stored,stored);
    assert.equal(JSON.parse(f.secrets.get('runtrol.runtime.integration.v1')).grant.integrationId,grant.integrationId);
  }finally{gate.resolve();f.close();}
});

for(const kind of ['providers','sessions']) {
  test(`cancelled ${kind} subscription cannot republish a queued snapshot`,async()=>{
    const f=fixture(), abort=new AbortController(), next=deferred(), snapshots=[];
    const method=kind==='providers'?'watchProvidersWithReconnect':'watchSessionIndexWithReconnect';
    f.client.connector[method]=async()=>({
      started:{snapshot:{marker:'initial'}},
      next:()=>next.promise,
      close:()=>next.resolve({kind:'ended',ended:{reason:'runtimeUnavailable'}}),
    });
    try{
      const watching=kind==='providers'
        ?f.client.watchProviders(value=>snapshots.push(value.marker),abort.signal)
        :f.client.watchSessions(value=>snapshots.push(value.marker),abort.signal);
      await until(()=>snapshots.length===1);
      next.resolve({kind:'changed',changed:{snapshot:{marker:'cancelled'}}});
      abort.abort();
      await watching;
      assert.deepEqual(snapshots,['initial']);
    }finally{abort.abort();f.close();}
  });
}

test('Studio initial window registration and handoff preserve the exact old mirror feeder and reveal registration',async()=>{
  const f=fixture();
  try{
    f.client.setWindowOwner(f.owner);
    const old=await f.client.publishWindow(f.state);
    const mirror=await old.openMirror({windowSessionId:'window',terminalKey:'shell',executionId:'execution',providerId:'test',workspace:'workspace',geometry:{columns:80,rows:24}});
    await mirror.output('YWJj');
    f.move();
    const current=await f.client.publishWindow(f.state);
    assert.notEqual(current,old);
    assert.equal(old.signal.aborted,false);
    await mirror.output('ZGVm');
    await mirror.end(0);
    assert.equal(f.output.length,2);
    assert.equal(f.output.every(entry=>entry.locator.revision===f.a.revision),true);
    assert.equal(f.ended[0].locator.revision,f.a.revision);
    const reveal=await f.client.revealAtOwner({windowSessionId:'other',terminalKey:'old-shell'},f.a.digest);
    assert.equal(reveal.delivered,true);
    assert.equal(old.signal.aborted,false);
  }finally{f.close();}
  assert.equal(f.connections.every(connection=>connection.closed>0),true);
});
