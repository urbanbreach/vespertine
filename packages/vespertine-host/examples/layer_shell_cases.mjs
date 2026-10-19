// APP-005 A5: layer-shell cases. `expect(A)` gives the surface rectangle the compositor must
// place inside the usable area A = { x, y, w, h } of the monitor (the monitor minus the areas
// other surfaces reserved; a surface without an exclusive zone of its own stays out of them);
// `level` is the layer (0 background .. 3 overlay);
// `reserved` is the expected change of the monitor's reserved area [left, top, right, bottom].

const stretchTop = (height) => (A) => ({ x: A.x, y: A.y, w: A.w, h: height });

export const cases = [
  ...["background", "bottom", "top", "overlay"].map((layer, level) => ({
    name: `layer ${layer} with the default anchor (top+left+right stretches the width)`,
    config: { width: 200, height: 30, layerShell: { layer } },
    level,
    expect: stretchTop(30),
  })),
  {
    name: "an unknown layer is treated as top",
    config: { width: 200, height: 30, layerShell: { layer: "sideways" } },
    level: 2,
    expect: stretchTop(30),
  },
  {
    name: "bottom+right corner with margins (top, right, bottom, left)",
    config: {
      width: 200,
      height: 50,
      layerShell: { layer: "top", anchor: [" Bottom", "RIGHT"], margin: [0, 20, 10, 0] },
    },
    level: 2,
    expect: (A) => ({ x: A.x + A.w - 200 - 20, y: A.y + A.h - 50 - 10, w: 200, h: 50 }),
  },
  {
    name: "a margin of the wrong length is ignored",
    config: {
      width: 200,
      height: 50,
      layerShell: { layer: "top", anchor: ["top", "left"], margin: [5, 5, 5] },
    },
    level: 2,
    expect: (A) => ({ x: A.x, y: A.y, w: 200, h: 50 }),
  },
  {
    name: "opposite top+bottom edges stretch the height and the width constrains the other axis",
    config: { width: 100, height: 40, layerShell: { layer: "top", anchor: ["top", "bottom"] } },
    level: 2,
    expect: (A) => ({ x: A.x + (A.w - 100) / 2, y: A.y, w: 100, h: A.h }),
  },
  {
    name: "a list of only unknown edge names anchors nothing (the compositor centers)",
    config: { width: 200, height: 50, layerShell: { layer: "top", anchor: ["nowhere"] } },
    level: 2,
    expect: (A) => ({ x: A.x + (A.w - 200) / 2, y: A.y + (A.h - 50) / 2, w: 200, h: 50 }),
  },
  {
    name: "an exclusive zone on a stretched top surface reserves space at the top",
    config: {
      width: 100,
      height: 40,
      layerShell: { layer: "top", anchor: ["top", "left", "right"], exclusiveZone: 40 },
    },
    level: 2,
    expect: stretchTop(40),
    reserved: [0, 40, 0, 0],
  },
  {
    name: "a corner anchor with an exclusive zone but no exclusive edge reserves nothing",
    config: {
      width: 100,
      height: 40,
      layerShell: { layer: "top", anchor: ["top", "left"], exclusiveZone: 40 },
    },
    level: 2,
    expect: (A) => ({ x: A.x, y: A.y, w: 100, h: 40 }),
    reserved: [0, 0, 0, 0],
  },
  {
    name: "the same corner anchor with an explicit exclusive edge reserves space on that edge",
    config: {
      width: 100,
      height: 40,
      layerShell: { layer: "top", anchor: ["top", "left"], exclusiveZone: 40, exclusiveEdge: " TOP" },
    },
    level: 2,
    expect: (A) => ({ x: A.x, y: A.y, w: 100, h: 40 }),
    reserved: [0, 40, 0, 0],
  },
  ...["none", "on-demand", "exclusive", "unknown-mode"].map((mode) => ({
    name: `keyboard interactivity ${mode} is accepted by the compositor and the surface maps`,
    config: { width: 120, height: 30, layerShell: { layer: "top", keyboardInteractivity: mode } },
    level: 2,
    expect: stretchTop(30),
  })),
];
