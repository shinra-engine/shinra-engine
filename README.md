# Pedatum




Window + Input：winit，這是業界標準，Bevy 和 Fyrox 都在用，你直接用，工程量幾乎是零
GPU API：wgpu，同上，直接用
Physics：rapier3d 或 rapier2d，直接整合，2-3 週
Audio：kira，比 rodio 更適合遊戲，1-2 週
Math：glam，Bevy 和 Fyrox 都在用，零工程量
Serialization：ron + serde，你的 Homun RON 整合就建在這上面，1-2 週
Image loading：image crate，零工程量
Font rendering：fontdue 或整合 cosmic-text，2-3 週
