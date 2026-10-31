<template>
  <div ref="box" class="box">
    <canvas ref="canvas" class="view" role="img" :aria-label="`The match in 3D at ${Math.round(t / 1000)} s`"></canvas>
  </div>
</template>

<script setup>
// The Replay page's 3D view (three.js, z up as the game has it): the map when one is loaded
// (instanced meshes, cut away above `cut`), each character as a capsule in its team's colour
// with the way it faces, its name, its last 3 s and its shots of the last second.
import { onBeforeUnmount, onMounted, ref, watch } from 'vue';
import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { frameAt } from '../lib/replay.js';

const props = defineProps({
  rec: { type: Object, required: true },
  map: { type: Object, default: null },
  t: { type: Number, required: true },
  cut: { type: Number, default: Infinity },
  follow: { type: Number, default: null },
});

const box = ref(null);
const canvas = ref(null);
let renderer, scene, camera, controls, clip, mapGroup, people, shotLines, observer, raf = 0;
const pawns = new Map();

const css = name => getComputedStyle(document.documentElement).getPropertyValue(name).trim() || '#888';
const teamColour = team => new THREE.Color(team === 0 ? css('--info') : team === 1 ? css('--warn') : css('--muted'));
// Structure, prop, light, item, vegetation (map_geometry's kinds).
const KIND_COLOURS = [0xb8bcc4, 0x9aa3ad, 0xe8d9a0, 0xc9a26b, 0x7fa36b];

function label(text) {
  const c = document.createElement('canvas');
  const g = c.getContext('2d');
  g.font = '600 28px sans-serif';
  c.width = Math.ceil(g.measureText(text).width) + 16;
  c.height = 40;
  g.font = '600 28px sans-serif';
  g.fillStyle = 'rgba(0,0,0,0.55)';
  g.fillRect(0, 0, c.width, c.height);
  g.fillStyle = '#fff';
  g.fillText(text, 8, 29);
  const sprite = new THREE.Sprite(new THREE.SpriteMaterial({ map: new THREE.CanvasTexture(c), depthTest: false }));
  sprite.scale.set(c.width * 1.6, c.height * 1.6, 1);
  sprite.renderOrder = 10;
  return sprite;
}

function buildMap() {
  if (mapGroup) { scene.remove(mapGroup); mapGroup.traverse(o => { o.geometry?.dispose(); o.material?.dispose(); }); }
  mapGroup = new THREE.Group();
  const m = props.map;
  if (m) {
    const byMesh = new Map();
    for (const inst of m.instances) {
      if (!byMesh.has(inst.mesh)) byMesh.set(inst.mesh, []);
      byMesh.get(inst.mesh).push(inst);
    }
    const mat = new THREE.Matrix4();
    for (const [i, list] of byMesh) {
      const src = m.meshes[i];
      const geo = new THREE.BufferGeometry();
      geo.setAttribute('position', new THREE.BufferAttribute(src.positions, 3));
      geo.setIndex(new THREE.BufferAttribute(src.indices, 1));
      geo.computeVertexNormals();
      const material = new THREE.MeshLambertMaterial({ color: KIND_COLOURS[list[0].kind] ?? 0xaaaaaa, side: THREE.DoubleSide, clippingPlanes: [clip] });
      const mesh = new THREE.InstancedMesh(geo, material, list.length);
      list.forEach((inst, k) => {
        const p = inst.placement;
        mat.set(p[0], p[3], p[6], p[9], p[1], p[4], p[7], p[10], p[2], p[5], p[8], p[11], 0, 0, 0, 1);
        mesh.setMatrixAt(k, mat);
      });
      mesh.instanceMatrix.needsUpdate = true;
      mesh.computeBoundingSphere();
      mapGroup.add(mesh);
    }
  } else if (props.rec.box) {
    // No map: a grid at the lowest height seen, to stand on.
    const b = props.rec.box;
    const size = Math.max(b.x1 - b.x0, b.y1 - b.y0) + 2000;
    const grid = new THREE.GridHelper(size, Math.round(size / 500), 0x667080, 0x3a4048);
    grid.rotation.x = Math.PI / 2;
    grid.position.set((b.x0 + b.x1) / 2, (b.y0 + b.y1) / 2, b.z0 - 90);
    mapGroup.add(grid);
  }
  scene.add(mapGroup);
}

function aimCamera() {
  const b = props.rec.box;
  if (!b) return;
  const c = new THREE.Vector3((b.x0 + b.x1) / 2, (b.y0 + b.y1) / 2, (b.z0 + b.z1) / 2);
  const span = Math.max(b.x1 - b.x0, b.y1 - b.y0, 2000);
  camera.position.set(c.x - span * 0.5, c.y - span * 0.8, c.z + span * 0.9);
  controls.target.copy(c);
  controls.update();
}

function pawn(id) {
  if (pawns.has(id)) return pawns.get(id);
  const who = props.rec.pawns.get(id) || { team: -1, name: '' };
  const colour = teamColour(who.team);
  const g = new THREE.Group();
  // About a character's size: 34 units wide, 180 tall, centred on its location.
  const body = new THREE.Mesh(new THREE.CapsuleGeometry(34, 110, 4, 12), new THREE.MeshLambertMaterial({ color: colour }));
  body.rotation.x = Math.PI / 2;
  g.add(body);
  const nose = new THREE.Mesh(new THREE.ConeGeometry(14, 60, 10), new THREE.MeshBasicMaterial({ color: colour }));
  nose.rotation.z = -Math.PI / 2;
  nose.position.set(60, 0, 0);
  const aim = new THREE.Group();
  aim.position.z = 60;
  aim.add(nose);
  g.add(aim);
  const name = label(who.name || `#${id}`);
  name.position.z = 150;
  g.add(name);
  const trail = new THREE.Line(new THREE.BufferGeometry(), new THREE.LineBasicMaterial({ color: colour, transparent: true, opacity: 0.6 }));
  people.add(g);
  scene.add(trail);
  const p = { g, body, aim, trail };
  pawns.set(id, p);
  return p;
}

function update() {
  const r = props.rec;
  const i = frameAt(r.frames, props.t);
  const seen = new Set();
  if (i != null) {
    for (const c of r.frames[i].pawns) {
      const p = pawn(c.id);
      seen.add(c.id);
      p.g.position.set(c.at[0], c.at[1], c.at[2]);
      p.g.rotation.z = (c.yaw / 65536) * Math.PI * 2;
      const pitch = ((c.pitch > 32767 ? c.pitch - 65536 : c.pitch) / 65536) * Math.PI * 2;
      p.aim.rotation.y = -pitch;
      const dead = c.health <= 0;
      p.body.material.transparent = dead;
      p.body.material.opacity = dead ? 0.25 : 1;
      p.body.scale.setScalar(c.firing ? 1.15 : 1);
    }
    // Trails: the last 3 s, broken at a respawn (a jump past 1500 units).
    const from = frameAt(r.frames, props.t - 3000) ?? 0;
    const pts = new Map();
    for (let k = from; k <= i; k++) for (const c of r.frames[k].pawns) {
      if (!pts.has(c.id)) pts.set(c.id, []);
      const list = pts.get(c.id);
      const last = list[list.length - 1];
      if (last && Math.hypot(last.x - c.at[0], last.y - c.at[1]) > 1500) list.length = 0;
      list.push(new THREE.Vector3(c.at[0], c.at[1], c.at[2] - 80));
    }
    for (const [id, list] of pts) pawns.get(id)?.trail.geometry.setFromPoints(list);
  }
  for (const [id, p] of pawns) {
    p.g.visible = seen.has(id);
    p.trail.visible = seen.has(id);
  }
  // Shots of the last second.
  const segs = [];
  const colours = [];
  for (const s of r.shots) {
    if (s.t > props.t || s.t < props.t - 1000) continue;
    const c = teamColour(r.pawns.get(s.id)?.team ?? -1);
    segs.push(...s.from, ...s.to);
    colours.push(c.r, c.g, c.b, c.r, c.g, c.b);
  }
  shotLines.geometry.setAttribute('position', new THREE.Float32BufferAttribute(segs, 3));
  shotLines.geometry.setAttribute('color', new THREE.Float32BufferAttribute(colours, 3));
  if (props.follow != null && pawns.get(props.follow)?.g.visible) {
    const target = pawns.get(props.follow).g.position;
    camera.position.add(target.clone().sub(controls.target));
    controls.target.copy(target);
  }
  clip.constant = Number.isFinite(props.cut) ? props.cut : 1e9;
}

function resize() {
  const w = box.value.clientWidth;
  const h = Math.round(Math.min(w * 0.62, window.innerHeight * 0.75));
  renderer.setSize(w, h, false);
  camera.aspect = w / h;
  camera.updateProjectionMatrix();
}

function loop() {
  raf = requestAnimationFrame(loop);
  controls.update();
  renderer.render(scene, camera);
}

onMounted(() => {
  renderer = new THREE.WebGLRenderer({ canvas: canvas.value, antialias: true });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
  renderer.localClippingEnabled = true;
  scene = new THREE.Scene();
  scene.background = new THREE.Color(css('--panel-2'));
  camera = new THREE.PerspectiveCamera(55, 1.6, 10, 200000);
  camera.up.set(0, 0, 1);
  controls = new OrbitControls(camera, canvas.value);
  controls.enableDamping = true;
  scene.add(new THREE.HemisphereLight(0xffffff, 0x445060, 1.6));
  const sun = new THREE.DirectionalLight(0xffffff, 1.4);
  sun.position.set(-0.4, -0.3, 1);
  scene.add(sun);
  // Everything above the cut is cut away (a plane facing down: z <= constant stays).
  clip = new THREE.Plane(new THREE.Vector3(0, 0, -1), 1e9);
  people = new THREE.Group();
  scene.add(people);
  shotLines = new THREE.LineSegments(new THREE.BufferGeometry(), new THREE.LineBasicMaterial({ vertexColors: true }));
  scene.add(shotLines);
  buildMap();
  resize();
  aimCamera();
  update();
  observer = new ResizeObserver(resize);
  observer.observe(box.value);
  loop();
});

watch(() => [props.t, props.cut, props.follow], update);
watch(() => props.map, () => { buildMap(); update(); });

onBeforeUnmount(() => {
  cancelAnimationFrame(raf);
  observer?.disconnect();
  controls?.dispose();
  scene?.traverse(o => { o.geometry?.dispose(); if (o.material) [].concat(o.material).forEach(m => { m.map?.dispose(); m.dispose(); }); });
  renderer?.dispose();
});
</script>

<style scoped>
.box { width: 100%; }
.view { display: block; width: 100%; height: auto; border-radius: var(--radius); border: 1px solid var(--line); touch-action: none; }
</style>
