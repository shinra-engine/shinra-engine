# Shinra Engine

## Design

```
             contract.wit
         data.rs + buffer.rs
        ↙        ↓       ↘
   data ──►  game.wasm ──┬──►  process.wasm ────►  data
  status     control    │    data pipeline      update
                ↓       │
            asset.wasm ──┴──►   render.wasm  ───►  buffer ──►  show
            appearance           graphs         targets

  └──── imperative ─────┘└─────── pure ───────┘
```

我們要設計一種全新的game engine IDE

目標是可以hotmodule替換 
0. contract.wit 定義資料形狀
1. 模型 asset1.wasm(model+texture), model2.wasm , ... 替換不同腳色讓遊戲繼續正常運作
2. data1.wasm(ex: ecs), data2.wasm,... 替換不同的data pipeline讓world運作中
3. render1.wasm, render2.wasm,.... 替換不同的render pipelin讓畫風不同(ex: 2d, 3d, pixel art)
4. game.wasm (替換不同的遊戲邏輯 但是game.wasm通常會操作render, data pipeline組合 比如透過替換game.wasm 再不用更新asset情況下從2d platform game變成jrpg 也行)

稱之為gametok 因為就像是短影音左滑一下就換了一款遊戲的感覺

我們需要同一個game repo compile 跨平台 android, web, win, apple,... 所以會需要高度一致性設計 ex: 是wgsl-native, 由GPU影響CPU排佈

並且因為wgsl-native 應該就會是 WebGPU-first的設計

因此 會是重協議的設計 ex: 只有遵守 data.rs + buffer.rs的 1234組件可以替換

反過來說 即使是hecs 必然是再data.so 將contract的資料 搬到自己定義的typeid中 執行完hecs 再轉回contract寫入如此複雜 因此 這個設計雖然跟ecs概念類似 卻跟hecs很難互融

在pc上使用winit + wasmtime

所有的game都需要引用contract.rs並且是#[repr(C)] 並且我們以遵守WGSL 的 std140/std430 為主

兩個wasm可以用shared memory溝通 這是一切的基礎

game.wasm也只是更新 contract.wit 裡面的control var sets

