<script setup lang="ts">
import { useToastStore } from "../stores/toast";
import Toast from "./Toast.vue";

const toastStore = useToastStore();
</script>

<template>
  <teleport to="body">
    <div class="toast-container">
      <TransitionGroup name="toast">
        <Toast v-for="t in toastStore.toasts" :key="t.id" :toast="t" />
      </TransitionGroup>
    </div>
  </teleport>
</template>

<style>
.toast-container {
  position: fixed;
  top: 16px;
  right: 16px;
  z-index: 99999;
  display: flex;
  flex-direction: column;
  gap: 10px;
  pointer-events: none;
}
.toast-container > * {
  pointer-events: auto;
}
.toast-enter-active,
.toast-leave-active {
  transition: opacity 0.22s ease, transform 0.22s ease;
}
.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translateX(24px);
}
.toast-leave-active {
  position: absolute;
  right: 0;
}
</style>
