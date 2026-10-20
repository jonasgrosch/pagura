import assert from 'node:assert/strict';
import { TileCache, TilePreloader, formsAllowPreload } from '../web/tile_cache.mjs';
import { tilesInRegion, aheadRegion } from '../web/tile_region.mjs';
const canvas = (width = 256, height = 256) => ({ width, height, remove() { this.detached = true; } });
const cost = 256 * 256 * 4, cache = new TileCache({ limit: cost * 3 });
const first = canvas(), second = canvas(), third = canvas(), fourth = canvas();
assert.ok(cache.put('a', first)); assert.ok(cache.put('b', second)); assert.ok(cache.put('c', third));
cache.pin(['a']); cache.get('b'); assert.ok(cache.put('d', fourth));
assert.equal(third.width, 0, 'least-recent non-visible backing store is explicitly released');
assert.equal(cache.bytes, cost * 3); assert.ok(cache.has('a')); assert.ok(cache.has('b'));
cache.pin(['a','b','d']); assert.equal(cache.put('e', canvas()), false, 'preloading cannot evict visible pixels or exceed the byte cap');
assert.equal(cache.put('huge', canvas(4096,4096)), false);
const tile = { x: 256, y: 0, width: 256, height: 128 };
assert.notEqual(TileCache.key(0,1,tile), TileCache.key(0,2,tile), 'different raster scales never alias');
assert.notEqual(TileCache.key(0,1,tile), TileCache.key(1,1,tile), 'pages never alias');
cache.clear(); assert.equal(cache.bytes, 0); assert.equal(cache.items.size, 0); assert.equal(first.width, 0);

const info = { width: 600, height: 1200 }, bounds = { left: 0, top: 0 }, viewport = { left: 0, top: 256, right: 600, bottom: 768 };
const down = tilesInRegion(info, 1, 1, 0, bounds, aheadRegion(viewport, { axis:'y', sign:1 }));
const up = tilesInRegion(info, 1, 1, 0, bounds, aheadRegion(viewport, { axis:'y', sign:-1 }));
assert.ok(down.every(tile => tile.y >= 768)); assert.ok(up.every(tile => tile.y < 256));
const right = aheadRegion(viewport, { axis:'x', sign:1 }); assert.equal(right.left, 600);
const rotated = tilesInRegion(info, 1, 1, 90, bounds, {left:0,top:0,right:256,bottom:256});
assert.ok(rotated.every(tile => tile.x === 0 && tile.y >= 768), 'rotated viewport maps back to source raster coordinates');
assert.equal(formsAllowPreload(true, null), false, 'unknown form metadata is conservative');
assert.equal(formsAllowPreload(true, new Map([[0,{}]])), false, 'active form filling never admits speculative tile work');
assert.equal(formsAllowPreload(true, new Map()), true);
assert.equal(formsAllowPreload(false, new Map([[0,{}]])), true);

let allowed = true, idle = true, time = 0, started = 0, admitted = 0, release;
const preloader = new TilePreloader({ allowed: () => allowed, idle: () => idle, now: () => time,
  render: async (_, current) => { started++; await new Promise(resolve => { release = resolve; }); if (!current()) return false; admitted++; return true; } });
preloader.schedule = () => {}; // Deterministic clock and dispatch, no sleeps.
preloader.plan(Array.from({length:100}, (_,i)=>i)); assert.equal(preloader.jobs.length,16);
await preloader.step(); assert.equal(started,0, 'idle delay applies before any render');
time=1000; idle=false; await preloader.step(); assert.equal(started,0, 'foreground queue drains first');
idle=true; const running = preloader.step(); await preloader.step(); assert.equal(started,1, 'only one background tile can be in flight');
preloader.cancel(); release(); await running; assert.equal(admitted,0); assert.equal(preloader.jobs.length,0);
preloader.plan([1]); allowed=false; time=2000; await preloader.step(); assert.equal(started,1, 'form focus/activity pauses dispatch');
allowed=true; const fresh = preloader.step(); release(); await fresh; assert.equal(admitted,1);
preloader.destroy(); preloader.plan([2]); await preloader.step(); assert.equal(started,2, 'closed viewers never resume background work');
console.log('Tile LRU/pinning, byte limits, scale keys, directional geometry and foreground/form priority passed');
