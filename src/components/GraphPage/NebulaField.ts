// 纯黑真空 + 远处稀疏静态星点 + 反射粒子云。
//
// 三层的亮度各有一条硬上限，分界就是 Bloom 的 threshold：
//   星体（节点/锚点）  自发光，越过阈值起一圈朦胧光晕——它是光源本身
//   连线星轨            芯部刚好越过阈值，晕开成淡光路
//   云团粒子            只反射附近星体的颜色，单颗最亮 ~0.04，恒在阈值之下
//
// 云那一条是封顶不是调出来的：一旦越过阈值就等于云自己在发光，
// 「接收 + 反射」和「发射」的区别就没了，星云退化成又一层星点。
//
// 世界尺度按场景外径等比放大（k = R / MOCK_RADIUS）。生产图谱从千级到万级
// 外径差一个数量级，不跟着放大的话小图云太厚、大图云缩成看不见的一团点。
//
// 色值走原始 sRGB（hex/255 直接进顶点属性），不走 THREE.Color：着色器是自己写的，
// 不含 colorspace_fragment，任何自动转换都只会让设计稿里定的色值漂一层。
// 全局关掉 THREE.ColorManagement 更是下策——那是整个进程的单例开关。

import * as THREE from "three";
import { mulberry32 } from "./graph-force3d";

const SEED = 0x5ee71a;

/** 视觉小样定稿时的构图外径，本文件的硬编码尺度常量全部以它为基准 */
const MOCK_RADIUS = 258;

/** hex → 原始 sRGB 的 0..1 三元组（不经 ColorManagement，与设计稿色值逐位一致） */
export function hexToRgb01(hex: string): [number, number, number] {
  const h = hex.replace("#", "");
  return [
    parseInt(h.slice(0, 2), 16) / 255,
    parseInt(h.slice(2, 4), 16) / 255,
    parseInt(h.slice(4, 6), 16) / 255,
  ];
}

/** 点云材质：k 是「世界尺度 → 屏幕像素」的换算系数 */
function pointMaterial(k: number): THREE.ShaderMaterial {
  return new THREE.ShaderMaterial({
    transparent: true,
    depthWrite: false,
    blending: THREE.AdditiveBlending,
    uniforms: { uPR: { value: 1 } },
    vertexShader: /* glsl */ `
      attribute float aSize; attribute float aAlpha;
      uniform float uPR; varying float vA;
      void main() {
        vA = aAlpha;
        vec4 mv = modelViewMatrix * vec4(position, 1.0);
        gl_PointSize = aSize * uPR * (${k.toFixed(1)} / max(-mv.z, 1.0));
        gl_Position = projectionMatrix * mv;
      }
    `,
    fragmentShader: /* glsl */ `
      varying float vA;
      void main() {
        float r = length(gl_PointCoord - 0.5);
        if (r > 0.5) discard;
        float a = smoothstep(0.5, 0.0, r);
        gl_FragColor = vec4(vec3(0.80, 0.86, 1.0) * a, a * a * vA);
      }
    `,
  });
}

/** 远处稀疏静态星点：只负责把黑色宇宙填出纵深，自己不该抢戏 */
export class Starfield {
  private readonly stars: THREE.Points;
  private readonly starMat: THREE.ShaderMaterial;

  constructor(opts: { sceneRadius: number; count?: number; pixelRatio?: number }) {
    const { sceneRadius, count = 1100, pixelRatio = 1 } = opts;
    const k = sceneRadius / MOCK_RADIUS;
    const rand = mulberry32(SEED);
    this.starMat = pointMaterial(3000 * k);
    this.starMat.uniforms.uPR.value = pixelRatio;

    // 三层视差：越远越小越暗。相机一转，三层的位移差就是纵深
    const pos = new Float32Array(count * 3);
    const size = new Float32Array(count);
    const alpha = new Float32Array(count);
    for (let i = 0; i < count; i++) {
      const layer = i % 3;
      const r = (780 + layer * 560 + rand() * 460) * k;
      const th = rand() * Math.PI * 2;
      const ph = Math.acos(2 * rand() - 1);
      pos[i * 3] = r * Math.sin(ph) * Math.cos(th);
      pos[i * 3 + 1] = r * Math.cos(ph);
      pos[i * 3 + 2] = r * Math.sin(ph) * Math.sin(th);
      size[i] = (1.5 - layer * 0.4) * (0.5 + rand());
      // 压在 Bloom 阈值之下：星点是填充，不是光源
      alpha[i] = (0.16 + layer * 0.09) * (0.3 + rand() * 0.7);
    }
    const geo = new THREE.BufferGeometry();
    geo.setAttribute("position", new THREE.BufferAttribute(pos, 3));
    geo.setAttribute("aSize", new THREE.BufferAttribute(size, 1));
    geo.setAttribute("aAlpha", new THREE.BufferAttribute(alpha, 1));
    this.stars = new THREE.Points(geo, this.starMat);
  }

  addTo(scene: THREE.Scene): void {
    scene.add(this.stars);
  }

  setPixelRatio(pr: number): void {
    this.starMat.uniforms.uPR.value = pr;
  }

  dispose(): void {
    this.stars.geometry.dispose();
    this.starMat.dispose();
  }
}

/** 每个锚点一团的粒子数 */
const CLOUD_PER = 320;
/** 反射强度上限：云把附近星体的色号乘这么高就往顶点属性里写 */
const CLOUD_REFLECT = 0.48;
/** 采样半径（小样尺度）：多远内的星体算这团云的反射源 */
const CLOUD_REACH = 190;
/** 采样权重的软化项（距离²，小样尺度）：太近也不让单颗星体主宰一整团云 */
const CLOUD_SOFTEN = 1400;
/** 云半径 = 罩住的星体群铺开尺度 × 系数，再夹进这个区间（小样尺度） */
const CLOUD_FIT_SCALE = 1.45;
const CLOUD_FIT_MIN = 40;
const CLOUD_FIT_MAX = 132;
/** 密度补偿的参考半径（小样尺度），见 fitCloud */
const CLOUD_DENS_REF = 64;
/** 云半径变化超过这个值（小样尺度）才重建散布形状 */
const CLOUD_FIT_EPS = 5;

/**
 * 星云云团：不发光，只反射。
 *
 * 粒子挂在结构锚点上——每个云团正好罩住一团同类星体，颜色才有得可反射；
 * 色号按 1/(d²+软化) 加权平均附近星体实时算出，所以切「标签/成员/状态」时
 * 云色跟着一起变。半径则按它罩住的星体群实际铺开多大自适应：锚点全挤在
 * 中部，半径写死的话几十团云会连成一片中央亮雾，把节点全埋进去。
 */
export class CloudField {
  private readonly points: THREE.Points;
  private readonly geo: THREE.BufferGeometry;
  private readonly mat: THREE.ShaderMaterial;
  private readonly anchors: number[];
  /** 参与反射采样的节点下标（只取任务节点，锚点是中性骨架，不该给云上色） */
  private readonly samplable: number[];
  private readonly k: number;
  /** 相对锚点的散布偏移，锚点动云跟着动 */
  private readonly off: Float32Array;
  private readonly pos: Float32Array;
  private readonly col: Float32Array;
  private readonly alpha: Float32Array;
  /** 未做密度补偿的基础亮度 */
  private readonly base: Float32Array;
  private readonly seed: Float32Array;
  private readonly radii: Float32Array;
  private readonly per: number;

  constructor(opts: {
    anchors: number[];
    samplable: number[];
    sceneRadius: number;
    pixelRatio?: number;
    perCluster?: number;
  }) {
    const { anchors, samplable, sceneRadius, pixelRatio = 1, perCluster = CLOUD_PER } = opts;
    this.anchors = anchors;
    this.samplable = samplable;
    this.k = sceneRadius / MOCK_RADIUS;

    const total = anchors.length * perCluster;
    this.per = perCluster;
    this.off = new Float32Array(total * 3);
    this.pos = new Float32Array(total * 3);
    this.col = new Float32Array(total * 3);
    this.alpha = new Float32Array(total);
    this.base = new Float32Array(total);
    this.seed = new Float32Array(total);
    this.radii = new Float32Array(anchors.length).fill(-1);

    // 散布形状不在这里撒：它归 fitCloud 用固定种子重建，这里只出与半径无关的属性
    const rand = mulberry32(SEED ^ 0x9e37);
    for (let i = 0; i < total; i++) {
      this.base[i] = 0.024 + rand() * 0.046;
      this.alpha[i] = this.base[i];
      this.seed[i] = rand();
    }

    const size = new Float32Array(total);
    for (let i = 0; i < total; i++) size[i] = 7 + rand() * 15;

    // 噪声频率随尺度反比、扰动幅度随尺度正比：这两者不跟着放大，
    // 大图的云会碎成一粒粒芝麻而不是一整片雾
    const noiseFreq = (0.009 / this.k).toFixed(6);
    const noiseDisp = (15 * this.k).toFixed(3);
    // 点径系数不乘 k：机位本身按 R 缩放（≈2.1R），点径里再乘一次就等于把
    // 粒子相对云胀大一圈，几团云一叠就顶过 Bloom 阈值自己发起光来
    const pointK = "2400.0";

    this.mat = new THREE.ShaderMaterial({
      transparent: true,
      depthWrite: false,
      blending: THREE.AdditiveBlending,
      uniforms: { uPR: { value: pixelRatio }, uTime: { value: 0 } },
      vertexShader: /* glsl */ `
        attribute vec3 aColor; attribute float aSize; attribute float aAlpha; attribute float aSeed;
        uniform float uPR; uniform float uTime;
        varying vec3 vC; varying float vA;
        float hash(vec3 p3) {
          p3 = fract(p3 * vec3(0.1031, 0.1030, 0.0973));
          p3 += dot(p3, p3.yxz + 33.33);
          return fract((p3.x + p3.y) * p3.z);
        }
        float vnoise(vec3 x) {
          vec3 i = floor(x), f = fract(x);
          f = f * f * (3.0 - 2.0 * f);
          return mix(mix(mix(hash(i), hash(i + vec3(1,0,0)), f.x),
                         mix(hash(i + vec3(0,1,0)), hash(i + vec3(1,1,0)), f.x), f.y),
                     mix(mix(hash(i + vec3(0,0,1)), hash(i + vec3(1,0,1)), f.x),
                         mix(hash(i + vec3(0,1,1)), hash(i + vec3(1,1,1)), f.x), f.y), f.z);
        }
        void main() {
          vC = aColor;
          vec3 q = position * ${noiseFreq}
                 + vec3(aSeed * 23.0, aSeed * 11.0, aSeed * 7.0)
                 + vec3(0.0, uTime * 0.02, 0.0);
          float n = vnoise(q) - 0.5;
          // 噪声扰动：粒子忽疏忽密地推来推去，云边就没有硬边
          vec3 wp = position + vec3(n, fract(n * 5.7) - 0.5, fract(n * 9.1) - 0.5) * ${noiseDisp};
          vec4 mv = modelViewMatrix * vec4(wp, 1.0);
          gl_PointSize = aSize * uPR * (${pointK} / max(-mv.z, 1.0));
          vA = aAlpha * (0.3 + (n + 0.5));
          gl_Position = projectionMatrix * mv;
        }
      `,
      fragmentShader: /* glsl */ `
        varying vec3 vC; varying float vA;
        void main() {
          float r = length(gl_PointCoord - 0.5);
          if (r > 0.5) discard;
          float a = smoothstep(0.5, 0.0, r); a *= a;
          gl_FragColor = vec4(vC * a, a * vA);
        }
      `,
    });

    this.geo = new THREE.BufferGeometry();
    this.geo.setAttribute("position", new THREE.BufferAttribute(this.pos, 3));
    this.geo.setAttribute("aColor", new THREE.BufferAttribute(this.col, 3));
    this.geo.setAttribute("aSize", new THREE.BufferAttribute(size, 1));
    this.geo.setAttribute("aAlpha", new THREE.BufferAttribute(this.alpha, 1));
    this.geo.setAttribute("aSeed", new THREE.BufferAttribute(this.seed, 1));
    this.points = new THREE.Points(this.geo, this.mat);
    // 顶点在 CPU 侧每帧改写，包围球必然过期，交给剔除只会让云随机消失
    this.points.frustumCulled = false;
  }

  addTo(scene: THREE.Scene): void {
    scene.add(this.points);
  }

  setPixelRatio(pr: number): void {
    this.mat.uniforms.uPR.value = pr;
  }

  /** 按已存的种子重建某组云的散布形状。
   *  用种子派生方向而不是另抽随机数：同一组每次重建形状完全一致，
   *  筛选来回切的时候云不会每次都换一副脸。 */
  private fitCluster(g: number, radius: number): void {
    this.radii[g] = radius;
    // 半径一小粒子就挤密，加色叠加是逐粒累加的，密度上去了云会自己顶过
    // Bloom 阈值开始发光——那正好把「云只反射、不发光」这条命门拆了。
    // 补偿只按 r^-0.8 压一部分差异：全按面积反比摊掉的话大半径的云会淡到
    // 看不见，而锚点铺得开的时候大半云都是大半径。
    const dens = Math.min(Math.pow((CLOUD_DENS_REF * this.k) / radius, 0.8), 1.45);
    for (let k2 = 0; k2 < this.per; k2++) {
      const i = g * this.per + k2;
      const s = this.seed[i];
      const th = s * Math.PI * 2;
      const ph = Math.acos(2 * ((s * 7.13) % 1) - 1);
      // 幂次 <1：中心密外围疏，叠上着色器噪声后边缘没有硬轮廓
      const rr = radius * (0.22 + 0.78 * Math.pow((s * 3.71) % 1, 0.62));
      this.off[i * 3] = rr * Math.sin(ph) * Math.cos(th);
      this.off[i * 3 + 1] = rr * Math.cos(ph) * 0.74;
      this.off[i * 3 + 2] = rr * Math.sin(ph) * Math.sin(th);
      this.alpha[i] = this.base[i] * dens;
    }
    this.geo.attributes.aAlpha.needsUpdate = true;
  }

  /**
   * 重算云色与半径：低频调用，只在调色板或筛选变过时。
   * 位置跟随是每帧的事（follow），两者频率差三个数量级，
   * 混在一起就是每帧重算几十团 × 上万个节点的加权平均。
   */
  sample(
    positions: Float32Array,
    colors: Array<readonly [number, number, number]>,
  ): void {
    const reach = CLOUD_REACH * this.k;
    const reach2 = reach * reach;
    const soften = CLOUD_SOFTEN * this.k * this.k;
    const lo = CLOUD_FIT_MIN * this.k;
    const hi = CLOUD_FIT_MAX * this.k;
    const eps = CLOUD_FIT_EPS * this.k;

    for (let g = 0; g < this.anchors.length; g++) {
      const ai = this.anchors[g];
      const ax = positions[ai * 3];
      const ay = positions[ai * 3 + 1];
      const az = positions[ai * 3 + 2];
      let r = 0;
      let g2 = 0;
      let b = 0;
      let w = 0;
      let dw = 0;
      for (const i of this.samplable) {
        const dx = positions[i * 3] - ax;
        const dy = positions[i * 3 + 1] - ay;
        const dz = positions[i * 3 + 2] - az;
        const d2 = dx * dx + dy * dy + dz * dz;
        if (d2 > reach2) continue;
        const weight = 1 / (d2 + soften);
        const c = colors[i];
        r += c[0] * weight;
        g2 += c[1] * weight;
        b += c[2] * weight;
        w += weight;
        dw += weight * Math.sqrt(d2);
      }
      const fit = w > 0 ? Math.min(Math.max((dw / w) * CLOUD_FIT_SCALE, lo), hi) : lo * 1.4;
      if (Math.abs(fit - this.radii[g]) > eps) this.fitCluster(g, fit);

      // 多色星体加权平均会洗成灰白，把色度按亮度推回去，云才带得住局部色调
      let cr = w > 0 ? r / w : 0.06;
      let cg = w > 0 ? g2 / w : 0.07;
      let cb = w > 0 ? b / w : 0.1;
      const lum = cr * 0.2126 + cg * 0.7152 + cb * 0.0722;
      cr = Math.max(0, lum + (cr - lum) * 1.3) * CLOUD_REFLECT;
      cg = Math.max(0, lum + (cg - lum) * 1.3) * CLOUD_REFLECT;
      cb = Math.max(0, lum + (cb - lum) * 1.3) * CLOUD_REFLECT;
      for (let k2 = 0; k2 < this.per; k2++) {
        const i = (g * this.per + k2) * 3;
        this.col[i] = cr;
        this.col[i + 1] = cg;
        this.col[i + 2] = cb;
      }
    }
    this.geo.attributes.aColor.needsUpdate = true;
  }

  /** 把粒子贴回锚点当前坐标：锚点被力场推着走，云得跟上 */
  follow(positions: Float32Array): void {
    for (let g = 0; g < this.anchors.length; g++) {
      const ai = this.anchors[g];
      const ax = positions[ai * 3];
      const ay = positions[ai * 3 + 1];
      const az = positions[ai * 3 + 2];
      for (let k2 = 0; k2 < this.per; k2++) {
        const i = (g * this.per + k2) * 3;
        this.pos[i] = ax + this.off[i];
        this.pos[i + 1] = ay + this.off[i + 1];
        this.pos[i + 2] = az + this.off[i + 2];
      }
    }
    this.geo.attributes.position.needsUpdate = true;
  }

  update(timeSec: number): void {
    this.mat.uniforms.uTime.value = timeSec;
  }

  dispose(): void {
    this.geo.dispose();
    this.mat.dispose();
  }
}
