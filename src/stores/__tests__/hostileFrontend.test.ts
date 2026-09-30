import {beforeEach, describe, expect, it, vi} from 'vitest';
import {createElement} from 'react';
import {renderToStaticMarkup} from 'react-dom/server';
import {useChatStore} from '../../stores/chatStore';
import {useNotebookStore} from '../../stores/notebookStore';
import {useSourceStore} from '../../stores/sourceStore';
import {useStudioStore} from '../../stores/studioStore';
import {useSettingsStore} from '../../stores/settingsStore';
import * as api from '../../lib/tauri';
import {SourceViewerModal} from '../../components/sources/SourceViewerModal';
import {SettingsDialog} from '../../components/settings/SettingsDialog';
import {EvidencePanel} from '../../components/inspector/EvidencePanel';
import {ChatPanel} from '../../components/chat/ChatPanel';
import {parseAssistantPayload} from '../../lib/chatEvidence';
// SSR's default Zustand getServerSnapshot returns initial state; explicitly project current state for these render probes.
vi.mock('zustand', async (importOriginal) => { const actual = await importOriginal<any>(); return { ...actual, create: (init:any) => { const original = actual.create(init); return Object.assign((selector:any = (state:any)=>state) => selector(original.getState()), original); } }; });
vi.mock('../../lib/tauri', () => ({
 createConversation: vi.fn(), listConversations: vi.fn().mockResolvedValue([]),
 loadMessages: vi.fn().mockResolvedValue([]), getChatEventsSince: vi.fn().mockResolvedValue([]),
 addSourcePaste: vi.fn(), listSources: vi.fn().mockResolvedValue([]),
 listNotebooks: vi.fn().mockResolvedValue([]), getNotebookStats: vi.fn(),
 exportStudioOutput: vi.fn(), listStudioOutputs: vi.fn(), stopChat: vi.fn(),
}));
vi.mock('react-virtuoso', () => ({Virtuoso: ({context, components}: any) => components?.Footer ? createElement(components.Footer, {context}) : null}));
vi.stubGlobal('localStorage', {getItem: () => null, setItem: () => {}, removeItem: () => {}});
function deferred<T>() { let resolve!: (v:T)=>void, reject!: (v:unknown)=>void; const promise = new Promise<T>((a,b)=>{resolve=a;reject=b}); return {promise,resolve,reject}; }
beforeEach(()=>{
 vi.clearAllMocks();
 useNotebookStore.setState({activeNotebookId:'nb-1',activationStatus:'confirmed'});
 useChatStore.setState({activeConversationId:'conv-1',conversations:[],messages:[],isStreaming:true,
 streamingNotebookId:'nb-1',streamingMessageId:'msg-1',preparingMessageId:null,streamingContent:'',
 streamingError:null,streamingStatus:null,streamReplayGap:false,pendingMessageIds:{'msg-1':true},pendingEvidence:{},replayCursors:{},lastChatEventSeq:0});
 useSourceStore.setState({sources:[], selectedSourceIds:new Set(),sourceScopeMode:'none',sourceListStatus:'error',loadedNotebookId:'nb-1',loadEpoch:0});
 useStudioStore.setState({outputs:[],activeOutputId:null,loadedNotebookId:'nb-1',status:'idle',error:null,activeGeneration:null});
 useSettingsStore.setState({settings:{}, models:[], featureFlags:[], providers:[], externalTools:{}});
});
const tokenEvent = (seq:number, token:string)=>({schema:'ChatStreamEventV1',seq,attempt_id:'msg-1',kind:'token',notebook_id:'nb-1',conversation_id:'conv-1',message_id:'msg-1',payload:{message_id:'msg-1',token}});
describe('hostile frontend acceptance probes on 66399d3',()=>{
 it('sequenced replay delivers repeated legitimate tokens once despite duplicate wakeups',async()=>{
  vi.mocked(api.getChatEventsSince).mockResolvedValueOnce([tokenEvent(2,'ha'),tokenEvent(1,'ha')] as any);
  await useChatStore.getState().replayChatEvents('nb-1','conv-1');
  vi.mocked(api.getChatEventsSince).mockResolvedValueOnce([tokenEvent(1,'ha'),tokenEvent(2,'ha')] as any);
  await useChatStore.getState().replayChatEvents('nb-1','conv-1');
  expect(useChatStore.getState().streamingContent).toBe('haha');
  expect(useChatStore.getState().replayCursors['nb-1:conv-1']).toBe(2);
 });
 it('overlapping wakeups serialize readers and apply each canonical sequence once',async()=>{
  const a=deferred<any[]>(),b=deferred<any[]>();
  vi.mocked(api.getChatEventsSince).mockReturnValueOnce(a.promise).mockReturnValueOnce(b.promise);
  const one=useChatStore.getState().replayChatEvents('nb-1','conv-1');
  const two=useChatStore.getState().replayChatEvents('nb-1','conv-1');
  expect(api.getChatEventsSince).toHaveBeenCalledTimes(1);
  a.resolve([tokenEvent(1,'A')]);
  await vi.waitFor(()=>expect(api.getChatEventsSince).toHaveBeenCalledTimes(2));
  expect(vi.mocked(api.getChatEventsSince).mock.calls[1][2]).toBe(1);
  b.resolve([tokenEvent(1,'A'),tokenEvent(2,'B')]);await Promise.all([one,two]);
  expect(useChatStore.getState().streamingContent).toBe('AB');
 });
 it('evicted history is disclosed and never concatenated as a complete prefix',async()=>{
  useChatStore.setState({streamingContent:'old prefix'});
  vi.mocked(api.getChatEventsSince).mockResolvedValueOnce([{...tokenEvent(5,''),kind:'gap',message_id:'',payload:{reason:'replay_history_evicted'}},tokenEvent(6,'tail')] as any);
  await useChatStore.getState().replayChatEvents('nb-1','conv-1');
  expect(useChatStore.getState().streamingContent).toBe('');
  expect(useChatStore.getState().streamingStatus?.phase).toBe('replay_gap');
 });
 it('replayed terminal closes retained notebook A owner while B is visible',async()=>{
  useNotebookStore.setState({activeNotebookId:'nb-2'});
  useChatStore.getState().resetForNotebookSwitch();
  vi.mocked(api.getChatEventsSince).mockResolvedValueOnce([{...tokenEvent(1,''),kind:'done'}] as any);
  await useChatStore.getState().replayChatEvents('nb-1','conv-1');
  expect(useChatStore.getState().isStreaming).toBe(false);
  expect(useChatStore.getState().messages).toEqual([]);
 });
 it('wrong payload identity cannot redirect a sequenced envelope',async()=>{
  vi.mocked(api.getChatEventsSince).mockResolvedValueOnce([{...tokenEvent(1,'wrong'),payload:{token:'wrong',message_id:'another'}}] as any);
  await useChatStore.getState().replayChatEvents('nb-1','conv-1');
  expect(useChatStore.getState().streamingContent).toBe('');
 });
 it('external New Chat is rejected without replacing an active stream',async()=>{
  await expect(useChatStore.getState().createConversation('nb-1')).rejects.toThrow('Stop the active response');
  expect(api.createConversation).not.toHaveBeenCalled();
  expect(useChatStore.getState().activeConversationId).toBe('conv-1');
 });
 it('F2-01 failed source paste must reject to preserve the component draft',async()=>{
  vi.mocked(api.addSourcePaste).mockRejectedValueOnce(new Error('disk full'));
  await expect(useSourceStore.getState().addSourcePaste('nb-1','Draft','unsaved research')).rejects.toThrow('disk full');
 });
 it('F4-03 late Studio export failure must not poison another notebook',async()=>{
  const op=deferred<any>();vi.mocked(api.exportStudioOutput).mockReturnValueOnce(op.promise);
  const pending=useStudioStore.getState().exportOutput('nb-1','output-1');
  useStudioStore.setState({loadedNotebookId:'nb-2',status:'idle',error:null});
  op.reject(new Error('old notebook export failed'));await pending;
  expect(useStudioStore.getState().error).toBeNull();expect(useStudioStore.getState().status).toBe('idle');
 });
 it('F4-04 older Studio load must not replace the latest same-notebook result',async()=>{
  const a=deferred<any[]>(),b=deferred<any[]>();
  vi.mocked(api.listStudioOutputs).mockReturnValueOnce(a.promise).mockReturnValueOnce(b.promise);
  const one=useStudioStore.getState().loadOutputs('nb-1');const two=useStudioStore.getState().loadOutputs('nb-1');
  b.resolve([{id:'new'}]);await two;a.resolve([{id:'old'}]);await one;
  expect(useStudioStore.getState().outputs[0].id).toBe('new');
 });
 it('F3-01 source viewer must have dialog semantics and a named close control',()=>{
  const markup=renderToStaticMarkup(createElement(SourceViewerModal,{notebookId:'nb-1',open:true,citation:{source_id:'s',source_title:'Source',chunk_id:'c',quote:'quote'} as any,onClose:()=>{}}));
  expect(markup).toContain('role="dialog"');expect(markup).toMatch(/aria-label="Close[^\"]*"/);
 });
 it('F3-02 settings must have dialog semantics and a named close control',()=>{
  const markup=renderToStaticMarkup(createElement(SettingsDialog,{open:true,onClose:()=>{}}));
  expect(markup).toContain('role="dialog"');expect(markup).toMatch(/aria-label="Close[^\"]*"/);
 });
 it('F2-02 latest assistant missing evidence must not expose older evidence as current',()=>{
  useChatStore.setState({isStreaming:false,messages:[{id:'older',role:'assistant',citations:parseAssistantPayload([])},{id:'newer',role:'assistant',content:'No captured evidence'}] as any});
  const markup=renderToStaticMarkup(createElement(EvidencePanel));
  expect(markup).toContain('No evidence available');expect(markup).not.toContain('Message: older');
 });
 it('F1-03 active stream text from notebook A must not render under notebook B',()=>{
  useChatStore.setState({streamingContent:'PRIVATE A ANSWER'});useNotebookStore.setState({activeNotebookId:'nb-2'});
  useChatStore.getState().resetForNotebookSwitch();
  const markup=renderToStaticMarkup(createElement(ChatPanel,{notebookId:'nb-2'}));
  expect(markup).not.toContain('PRIVATE A ANSWER');
 });
 it('positive control: degraded source list cannot block explicit no-retrieval',()=>{
  expect(useSourceStore.getState().getSourceScope()).toEqual({kind:'none'});
 });
});
