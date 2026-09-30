import {beforeEach, describe, expect, it, vi} from 'vitest';
import {isValidElement, type ReactNode} from 'react';
import {FlashcardWidget} from '../../components/studio/FlashcardWidget';
import {QuizWidget} from '../../components/studio/QuizWidget';
import {SourcesPanel} from '../../components/sources/SourcesPanel';
import {useSourceStore} from '../../stores/sourceStore';
import {useNotebookStore} from '../../stores/notebookStore';
import * as api from '../../lib/tauri';
const hook = vi.hoisted(()=>({slots:[] as any[],index:0}));
vi.mock('react',async(original)=>({...await original<typeof import('react')>(),
 useState:(initial:any)=>{const idx=hook.index++;if(!(idx in hook.slots))hook.slots[idx]=typeof initial==='function'?initial():initial;return[hook.slots[idx],(v:any)=>{hook.slots[idx]=typeof v==='function'?v(hook.slots[idx]):v}];},
 useRef:(initial:any)=>{const idx=hook.index++;return hook.slots[idx]??={current:initial}},
 useMemo:(fn:any)=>fn(),useCallback:(fn:any)=>fn,
}));
vi.mock('zustand',async(original)=>{const actual=await original<any>();return{...actual,create:(init:any)=>{const store=actual.create(init);return Object.assign((select:any=(s:any)=>s)=>select(store.getState()),store)}}});
vi.mock('../../lib/tauri',()=>({addSourcePaste:vi.fn(),retryFailedImports:vi.fn(),listSources:vi.fn().mockResolvedValue([]),listNotebooks:vi.fn().mockResolvedValue([]),getNotebookStats:vi.fn().mockResolvedValue({source_count:0})}));
vi.stubGlobal('localStorage',{getItem:()=>null,setItem:()=>{},removeItem:()=>{}});
function find(node:ReactNode,predicate:(props:any)=>boolean):any{if(Array.isArray(node))return node.map(n=>find(n,predicate)).find(Boolean);if(isValidElement(node)){const p=node.props as any;return predicate(p)?node:find(p.children,predicate)}}
function render(fn:any,props:any){hook.index=0;return fn(props)}
function deferred(){let resolve!:(v:any)=>void;const promise=new Promise<any>(r=>resolve=r);return{promise,resolve}}
function sourceTree(){return render(SourcesPanel,{notebookId:'nb-1'})}
function button(tree:any,label:string){return find(tree,p=>p.children===label)}
beforeEach(()=>{hook.slots=[];hook.index=0;vi.clearAllMocks();useNotebookStore.setState({activeNotebookId:'nb-1'});useSourceStore.setState({sources:[],selectedSourceIds:new Set(),sourceListStatus:'ready',loadedNotebookId:'nb-1',stats:null})});
describe('real callback state-transition probes',()=>{
 it('F4-05 replacing a progressed flashcard output with a shorter output must not crash',()=>{
  const out=(n:number)=>({id:String(n),raw_content:JSON.stringify(Array.from({length:n},(_,i)=>({front:`q${i}`,back:`a${i}`})))});
  let tree=render(FlashcardWidget,{output:out(3)});button(tree,'Know').props.onClick({stopPropagation(){}});
  tree=render(FlashcardWidget,{output:out(3)});button(tree,'Know').props.onClick({stopPropagation(){}});
  expect(()=>render(FlashcardWidget,{output:out(1)})).not.toThrow();
 });
 it('F4-05 replacing a progressed quiz output with a shorter output must not crash',()=>{
  const out=(n:number)=>({id:String(n),raw_content:JSON.stringify(Array.from({length:n},(_,i)=>({question:`q${i}`,options:['a','b'],correct_index:0})))});
  let tree=render(QuizWidget,{output:out(3)});find(tree,p=>Array.isArray(p.children)&&p.children[1]==='a').props.onClick();
  tree=render(QuizWidget,{output:out(3)});button(tree,'Next').props.onClick();
  expect(()=>render(QuizWidget,{output:out(1)})).not.toThrow();
 });
 it('F2-01 failed paste keeps actual component draft for retry',async()=>{
  // SourcesPanel slots 0,2,3 are showPaste,title,text at this snapshot.
  sourceTree();hook.slots[0]=true;hook.slots[2]='Draft';hook.slots[3]='Unsaved research';
  vi.mocked(api.addSourcePaste).mockRejectedValueOnce(new Error('disk full'));
  await button(sourceTree(),'Add Source').props.onClick();
  expect(hook.slots[3]).toBe('Unsaved research');expect(hook.slots[0]).toBe(true);
 });
 it('F4-06 typing a new paste while previous import awaits must not discard new text',async()=>{
  sourceTree();hook.slots[0]=true;hook.slots[2]='First';hook.slots[3]='First text';
  const op=deferred();vi.mocked(api.addSourcePaste).mockReturnValueOnce(op.promise);
  const pending=button(sourceTree(),'Add Source').props.onClick();
  find(sourceTree(),p=>p.placeholder==='Paste text here...').props.onChange({target:{value:'New unrelated unsaved text'}});
  op.resolve('source-id');await pending;
  expect(hook.slots[3]).toBe('New unrelated unsaved text');
 });
 it('F2-03 Retry All consumes handled backend rejection rather than leaking event promise',async()=>{
  useSourceStore.setState({sources:[{id:'s',title:'Failed',status:'error',source_type:'paste'}] as any});
  vi.mocked(api.retryFailedImports).mockRejectedValueOnce(new Error('retry unavailable'));
  await expect(button(sourceTree(),'Retry All').props.onClick()).resolves.toBeUndefined();
 });
});
