// 图谱材质：星体点云与星轨窄带的 glsl 着色器 + 两套材质工厂。
//
// 为什么单独一个文件：着色器源码是「档位表」与「渲染」之间唯一的接线点，
// 单测要能直接断言着色器里的字面量（点径上限、NDC↔像素换算系数、纵深收细
// 系数）——断言写在材质旁边才够得着，混在画布组件里就没人看得住了。
//
// 搬移是逐字的：常量、着色器源码与工厂函数原样保留、顺序不变。

import * as THREE from "three";
import type { GraphEdgeWidth } from "../../lib/graphPrefs";

/** 连线粗细三档（设置页），单位是名义 CSS 像素：窄带有了真实宽度，
 *  这一档就该真的调宽度。旧的 LineSegments 恒 1px 宽，只能拿 alpha 顶替，
 *  而加色混合会把 alpha 平方衰减——「细」档直接等于看不见。
 *  标准档 1.6px 就是小样的定稿宽度：这三个数就是像素值本身，不再乘任何基准，
 *  近景纵深上量出来就是这个宽度（纵深收细见 EDGE_DEPTH_*）。 */
export const EDGE_WIDTH: Record<GraphEdgeWidth, number> = {
  thin: 1.0,
  standard: 1.6,
  thick: 2.8,
};

/** 窄带按纵深收细：近景更粗、远景更细，密图远景才不会糊成一块板。
 *  参考纵深取小样的机位距离（2.882 × R258 ≈ 744），在这个纵深上系数正好是 1，
 *  于是 EDGE_WIDTH 就是字面意义的屏幕像素；越远越细，但不细过下限——
 *  亚像素的窄带在加色混合下等于看不见，那正是这档设置要避免的事。 */
export const EDGE_DEPTH_REF = 744;
export const EDGE_DEPTH_CLAMP_MIN = 0.5;
export const EDGE_DEPTH_CLAMP_MAX = 1.15;

/** NDC↔像素的换算系数：uRes 宽的视口横跨 2 个 NDC 单位，屏幕 1 像素 = 2/uRes。
 *  它以前是着色器里一个裸字面量，档位表测试够不着——常数改了测试照样全绿。
 *  提成常量插值进着色器，单测才有第二条真值可断言。 */
export const EDGE_NDC_SCALE = 2.0;

/** 节点点径系数：世界半径 → 屏幕直径（CSS 像素）的分子。
 *  提成常量插值进着色器，单测才有第二条真值可断言（档位表与着色器脱节过一次）。 */
export const NODE_POINT_SCALE = 2300;

/** 单个星点的屏幕直径上限（CSS 像素）：相机贴到节点跟前时点径会炸成巨斑 */
export const NODE_POINT_MAX_PX = 64;

const NODE_VERT = /* glsl */ `
  attribute vec3 aColor; attribute float aSize; attribute float aAlpha;
  uniform float uPR;
  varying vec3 vC; varying float vA;
  void main() {
    vC = aColor; vA = aAlpha;
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    // 点径上限：相机拉近到贴着节点走时，1/纵深 会把点撑成几百像素的巨斑
    gl_PointSize = min(aSize * uPR * (${NODE_POINT_SCALE.toFixed(1)} / max(-mv.z, 1.0)), ${NODE_POINT_MAX_PX.toFixed(1)} * uPR);
    gl_Position = projectionMatrix * mv;
  }
`;
const NODE_FRAG = /* glsl */ `
  varying vec3 vC; varying float vA;
  void main() {
    float r = length(gl_PointCoord - 0.5);
    if (r > 0.5) discard;
    float halo = pow(smoothstep(0.5, 0.0, r), 2.4);
    float core = smoothstep(0.17, 0.0, r);
    // 核心往本色里混而不是直接上纯白：加法混合下纯白心会把高连接度节点顶成一团
    // 过曝白斑，色相全丢——那正是「按状态着色」要靠颜色读出来的东西
    vec3 col = vC * halo * 1.30 + mix(vC, vec3(1.0), 0.62) * core * 0.62;
    gl_FragColor = vec4(col, (halo * 0.58 + core * 0.82) * vA);
  }
`;

/** 边：随相机转向的窄带，不是 1 像素线。
 *  LineSegments 恒为 1 像素宽，加色混合下存在感全靠 alpha，而 alpha 又被混和方式
 *  平方衰减，压在 Bloom 阈值下就成了「有数据没画面」。星轨要的是一条有粗细、有软芯的
 *  光路，所以每条边展开成 4 个角点的窄带：横截面 smoothstep 收出软边，芯部亮度
 *  刚好越过阈值起一点辉，边上就散成淡光；远处的边按深度收细，密图远景才不会糊成板。 */
/** 节点点云材质（导出成工厂：点径上限要在着色器源码上断言，
 *  档位表与着色器脱节过一次，单测够不着着色器就等于没测） */
export function createNodeMaterial(pixelRatio: number): THREE.ShaderMaterial {
  return new THREE.ShaderMaterial({
    transparent: true,
    depthWrite: false,
    blending: THREE.AdditiveBlending,
    uniforms: { uPR: { value: pixelRatio } },
    vertexShader: NODE_VERT,
    fragmentShader: NODE_FRAG,
  });
}

const EDGE_VERT = /* glsl */ `
  attribute vec3 aStart; attribute vec3 aEnd;
  attribute vec3 aColor; attribute float aAlpha; attribute vec2 aCorner;
  uniform vec2 uRes; uniform float uWidth;
  varying vec3 vC; varying float vA; varying float vS;
  void main() {
    vC = aColor; vA = aAlpha; vS = aCorner.x;
    vec4 cs = projectionMatrix * modelViewMatrix * vec4(aStart, 1.0);
    vec4 ce = projectionMatrix * modelViewMatrix * vec4(aEnd, 1.0);
    // 端点跑到相机背后时透视除法会把窄带拉成横贯全屏的巨块，直接塌掉
    if (cs.w <= 0.0 || ce.w <= 0.0) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); return; }
    vec2 dir = (ce.xy / ce.w - cs.xy / cs.w) * uRes;
    float len = length(dir);
    dir = len > 1e-4 ? dir / len : vec2(1.0, 0.0);
    // ×2 是 NDC↔像素的换算：uRes 宽的视口横跨 2 个 NDC 单位，
    //  漏掉这个系数，uWidth 标称的像素宽会只剩一半（实测三档全落进亚像素）
    vec2 nrm = vec2(-dir.y, dir.x) * ${EDGE_NDC_SCALE.toFixed(1)} / uRes;
    float depth = -(modelViewMatrix * vec4(aStart, 1.0)).z;
    vec4 p = mix(cs, ce, aCorner.y);
    p.xy += nrm * (aCorner.x * uWidth * 0.5 * clamp(${EDGE_DEPTH_REF.toFixed(1)} / max(depth, 1.0), ${EDGE_DEPTH_CLAMP_MIN}, ${EDGE_DEPTH_CLAMP_MAX})) * p.w;
    gl_Position = p;
  }
`;
const EDGE_FRAG = /* glsl */ `
  varying vec3 vC; varying float vA; varying float vS;
  void main() {
    float w = smoothstep(0.0, 1.0, 1.0 - abs(vS));
    gl_FragColor = vec4(vC * vA * w, w * vA);
  }
`;

/** 星轨材质。档位值从 EDGE_WIDTH 直接进 uWidth，着色器里只有 uWidth 一个宽度来源——
 *  单测靠它够到 uniform 接线（曾出现过「档位表是 1.6、着色器另乘一个基准」的整档错位）。 */
export function createEdgeMaterial(pref: GraphEdgeWidth): THREE.ShaderMaterial {
  return new THREE.ShaderMaterial({
    transparent: true,
    depthWrite: false,
    side: THREE.DoubleSide,
    blending: THREE.AdditiveBlending,
    uniforms: {
      uRes: { value: new THREE.Vector2(1, 1) },
      uWidth: { value: EDGE_WIDTH[pref] },
    },
    vertexShader: EDGE_VERT,
    fragmentShader: EDGE_FRAG,
  });
}
