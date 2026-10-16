# 麗京特別市 城市圖鑑（原生版）

《環》麗京特別市（舊平潭）的城市參考工具，給《城市天際線 2》重建用。
以 Rust + wgpu 直接驅動顯示卡（Windows 用 DirectX 12／Vulkan），不經過瀏覽器。

## 執行

解壓縮後雙擊 `lijing.exe`。`data` 資料夾必須和 exe 放在同一層。

## 操作

| 動作 | 方式 |
|---|---|
| 平移 | 左鍵拖曳；WASD／方向鍵 |
| 旋轉、傾斜 | 右鍵或中鍵拖曳；Shift／Ctrl／Alt＋左鍵拖曳；Q／E 旋轉、R／F 傾斜 |
| 左鍵改成轉視角 | 底部「左鍵：平移」按鈕或 T 鍵（上下拖曳傾斜、左右拖曳旋轉） |
| 縮放 | 滾輪（往游標位置）；＋／－ |
| 模式 | 1 平面、2 立體、3 等高線、4 運輸、5 CS2 規劃；M 量測；Esc 取消 |

- 時間軸：2014（戰前，Landsat 衛星影像判讀）→ 2015 接收 → 2016–2025 麗京建設，逐年顯示新建與拆除。
- 軌道路線預設隱藏，在右側面板點選路線開啟；運輸模式顯示全部。
- 地標與車站可開「內部剖面」，可展開樓層或只看單層。
- CS2 模式可直接把可建造範圍的 16-bit 高度圖存成檔案。
- 底部「解析度」：預設自動，FPS 低於 40 時會自動降低繪製解析度。

## 自動更新

程式啟動時會查詢 GitHub Releases 的最新版本，有新版本時畫面上方會出現提示，
按「下載並更新」→「立即重新啟動」即可。設定檔：`data/update.json`

```json
{ "repo": "擁有者/儲存庫", "asset": "lijing-windows.zip" }
```

## 發佈新版本（維護者）

1. 修改 `Cargo.toml` 的 `version`。
2. 推送標籤：`git tag v0.7.1 && git push origin v0.7.1`。
3. GitHub Actions（`.github/workflows/release.yml`）會在 Windows 上編譯並把 `lijing-windows.zip` 發佈到 Releases，
   使用者的程式下次啟動就會看到更新提示。

## 資料來源

- 地形：AWS Terrain Tiles（約 17 m），將軍山系依聖經抬升。
- 戰前（2014–15）道路與建物：Overture Maps（OpenStreetMap 道路、East Asian Building Footprints 建物）
  以 Landsat 8（2014–15 與 2020–21 同季節影像）判讀當時是否已存在。
- 麗京 2016–2025：以真實路網為骨架、在空地長出的規劃街廓與建物（程序生成）。
