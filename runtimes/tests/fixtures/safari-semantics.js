'use strict';
const sentinel = {message:'same thrown object'};
let generatorFinally = 0;
class Base { constructor(value) { this.value=value; } method() { return this.value; } }
class Derived extends Base {
  constructor(value) { super(value+1); }
  #triple(value) { return value*3; }
  get total() { return this.#triple(this.value); }
  method(add=2,...rest) { return super.method()+this.total+add+rest.length; }
}
function recur(n) { return n===0 ? 0 : recur(n-1)+1; }
function fail() { throw sentinel; }
function overrideFinally() { try { throw sentinel; } finally { return 17; } }
function strictThis() { return this===undefined; }
function* sequence() { try { yield 3; yield 5; } finally { generatorFinally++; } }
async function delayed(value) { await Promise.resolve(); return value+1; }
async function returnsRejected() { return Promise.reject(sentinel); }
globalThis.unrelated = function unrelated() { return 99; };
globalThis.run = async function run() {
  const object=new Derived(4);
  const lexical=()=>this;
  let caught=false;try { fail(); } catch(error) { caught=error===sentinel; }
  const iterator=sequence();const first=iterator.next().value;const returned=iterator.return(23).value;
  let rejection=false;try { await returnsRejected(); } catch(error) { rejection=error===sentinel; }
  return {method:object.method(2,1,2),total:object.total,recur:recur(4),caught,override:overrideFinally(),strict:strictThis(),lexical:lexical()===this,delayed:await delayed(6),first,returned,generatorFinally,rejection};
};
