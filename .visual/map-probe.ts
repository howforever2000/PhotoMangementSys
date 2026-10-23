import { createApp, h, ref } from "vue";
import "../src/assets/tokens.css";
import LocationMap from "../src/components/ui/LocationMap.vue";
import StatBand from "../src/components/ui/StatBand.vue";

// 真实城市坐标 + 模拟照片数（覆盖：超大城市 / 中等 / 极小 / 西部稀疏）
const groups = [
  { location: "北京市", count: 186, lat: 39.90, lon: 116.40 },
  { location: "上海市", count: 142, lat: 31.23, lon: 121.47 },
  { location: "成都市", count: 96, lat: 30.57, lon: 104.07 },
  { location: "广州市", count: 88, lat: 23.13, lon: 113.26 },
  { location: "杭州市", count: 61, lat: 30.27, lon: 120.15 },
  { location: "西安市", count: 44, lat: 34.34, lon: 108.94 },
  { location: "武汉市", count: 39, lat: 30.59, lon: 114.31 },
  { location: "昆明市", count: 27, lat: 25.04, lon: 102.71 },
  { location: "哈尔滨市", count: 22, lat: 45.80, lon: 126.53 },
  { location: "乌鲁木齐市", count: 15, lat: 43.83, lon: 87.62 },
  { location: "拉萨市", count: 8, lat: 29.65, lon: 91.14 },
  { location: "三亚市", count: 6, lat: 18.25, lon: 109.51 },
  { location: "厦门市", count: 5, lat: 24.48, lon: 118.09 },
  { location: "大理市", count: 3, lat: 25.61, lon: 100.27 },
  { location: "敦煌市", count: 1, lat: 40.14, lon: 94.66 },
];

const stat = [
  { label: "有地点城市", value: 15, delta: "已点亮" },
  { label: "GPS 覆盖", value: "94.2", unit: "%", delta: "615 / 653" },
  { label: "未记录", value: 38, delta: "无定位", tone: "mute" as const },
  { label: "最大聚集", value: 186, unit: "张", delta: "北京市" },
];

const hovered = ref<string | null>(null);
createApp({
  setup() {
    return () =>
      h("div", { style: "min-height:100vh;background:var(--c-bg);padding-top:28px" }, [
        h(StatBand, { items: stat }),
        h("div", { style: "height:1px;background:var(--c-rule);margin:18px 52px 0" }),
        h("div", { style: "padding:0 52px;display:flex;justify-content:center" }, [
          h(LocationMap, {
            groups,
            width: 720,
            activeName: hovered.value,
            onPick: (g: { location: string }) => (hovered.value = g.location),
          }),
        ]),
      ]);
  },
}).mount("#app");
