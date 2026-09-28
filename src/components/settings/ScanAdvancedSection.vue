<script setup lang="ts">
/**
 * ③ 高级选项 · 扫描批次
 *
 * 批次**不是硬件开关**（对识别腿无吞吐收益，后端 `clamp(4,64)` 默认 8），
 * 所以按约定收进「高级选项」折叠区，不再出现在两个扫描面板的可见行里。
 * 值存在 Pinia 的 `scanBatch`（localStorage 持久化），相册扫描与全局扫描共用一份。
 */
import { computed } from "vue";
import { BATCH_OPTIONS, useContentStore } from "../../stores/content";

const store = useContentStore();

const batch = computed({
  get: () => store.scanBatch,
  set: (n: number) => store.setScanBatch(n),
});
</script>

<template>
  <div class="adv-row">
    <label class="lbl" for="scan-batch">扫描批次</label>
    <select id="scan-batch" v-model.number="batch" class="sel">
      <option v-for="b in BATCH_OPTIONS" :key="b" :value="b">{{ b }} 张/批</option>
    </select>
    <span class="stat">每批一个识别请求 · 改动对下一次扫描生效</span>
  </div>
  <p class="hint">
    批次只影响<b>一次请求带几张图</b>：对人物/文档识别没有吞吐收益（默认 8 已是推荐值），
    真正要调的是上面的 <b>CPU 线程数</b> 与 <b>GPU 加速</b>。一般不用改；
    仅在内存吃紧或语义索引大批量编码时才需要往下调。
  </p>
</template>

<style scoped>
.adv-row {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.lbl {
  min-width: 76px;
  font-size: 12.5px;
  color: var(--color-text-2);
}
.sel {
  min-width: 150px;
  padding: 7px 10px;
  border-radius: var(--radius-sm, 8px);
  border: 1px solid var(--color-border);
  background: var(--color-surface-2);
  color: var(--color-text);
  font-size: 13px;
}
.sel:focus-visible {
  outline: 2px solid rgba(57, 108, 216, 0.55);
  outline-offset: 1px;
}
.stat {
  font-size: 12.5px;
  color: var(--color-text-3);
}
.hint {
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--color-text-3);
}
</style>
