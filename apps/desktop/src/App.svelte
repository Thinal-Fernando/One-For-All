<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";

  // The Rust side polls the cursor and tells us when to open, because a
  // click-through window never receives hover events of its own.
  let open = $state(false);
  let island: HTMLDivElement;

  onMount(() => {
    // Rust only treats the painted island as solid, so report its box
    // whenever it changes size, including every frame of the open animation.
    const report = () => {
      const box = island.getBoundingClientRect();
      invoke("set_hit_area", {
        x: box.x,
        y: box.y,
        width: box.width,
        height: box.height,
      });
    };
    const observer = new ResizeObserver(report);
    observer.observe(island);

    const unlisten = listen<boolean>("island-hover", (event) => {
      open = event.payload;
    });

    return () => {
      observer.disconnect();
      unlisten.then((stop) => stop());
    };
  });
</script>

<div class="island" class:open bind:this={island} aria-label="OFA island"></div>

<style>
  .island {
    width: 160px;
    height: 32px;
    margin-top: 6px;
    border-radius: 16px;
    background: #000;
    box-shadow: 0 0 0 1px rgba(255, 255, 255, 0.08);
    transition:
      width 220ms cubic-bezier(0.2, 0.9, 0.3, 1.1),
      height 220ms cubic-bezier(0.2, 0.9, 0.3, 1.1),
      border-radius 220ms ease;
  }

  .island.open {
    width: 360px;
    height: 120px;
    border-radius: 24px;
  }
</style>
