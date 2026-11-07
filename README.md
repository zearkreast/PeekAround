<p align="center">
  <img src="brand/logo.svg" width="112" alt="PeekAround logo — a black-browed albatross looking left">
</p>

<h1 align="center">PeekAround 2.0</h1>
<p align="center"><b>Klasörlerin, en güzel haliyle.</b> · <i>Your folders, beautifully.</i></p>

<p align="center">
  <a href="https://github.com/zearkreast/PeekAround/releases"><img src="https://img.shields.io/github/v/release/zearkreast/PeekAround?include_prereleases&label=%C3%B6n%20s%C3%BCr%C3%BCm&color=f97316" alt="Ön sürüm"></a>
  <a href="https://github.com/zearkreast/PeekAround/releases/latest"><img src="https://img.shields.io/github/v/release/zearkreast/PeekAround?label=kararl%C4%B1&color=8b5cf6" alt="Kararlı sürüm"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/zearkreast/PeekAround?label=lisans&color=0ea5e9" alt="Lisans: GPL-3.0"></a>
  <img src="https://img.shields.io/badge/Rust-Slint-b7410e" alt="Rust + Slint">
</p>

<p align="center">
  <a href="https://github.com/zearkreast/PeekAround/releases/tag/v2.0.0-alpha.1"><b>⬇️ Alfayı indir</b></a> ·
  <a href="#durum">Durum</a> ·
  <a href="#bu-sürümde-olanlar">Özellikler</a> ·
  <a href="#kurulum">Kurulum</a> ·
  <a href="#geliştirme">Geliştirme</a>
</p>

> 🦀 **Bu dal PeekAround'un Rust + Slint ile baştan yazımıdır ve alfa aşamasındadır.** Electron ve React yok, arayüz yerel olarak çizilir. Windows paketi 148 MB'tan 41 MB'a indi. Günlük kullanım için kararlı **1.x** sürümü [`main`](https://github.com/zearkreast/PeekAround/tree/main) dalında ve [Releases](https://github.com/zearkreast/PeekAround/releases/latest) sayfasında.

<p align="center">
  <img src="docs/screenshots/explore-2.0.jpg" width="90%" alt="2.0 Keşfet sayfası: anılar, geziler ve yıl özetleri">
</p>

PeekAround; bilgisayarındaki ya da sunucundaki klasörleri modern, animasyonlu bir galeriye dönüştürür. Fotoğraf ve videolarına göz at, izle, albümlere ayır, anılarını ve gezilerini otomatik bulsun; orijinal dosyalara hiç dokunmaz.

---

## Durum

| Aşama | Kapsam | Durum |
|---|---|---|
| M1 | Sunucu: tarama, küçük resim, meta veri, API, MEGA, telefon bağlantısı | ✅ |
| M2 | Masaüstü: galeri, görüntüleyici, seçim, kitaplık, ayarlar, tepsi | ✅ |
| M3 | Keşfet, anı oynatıcı ve harita | 🟡 büyük ölçüde |
| M4 | Düzenleyiciler: fotoğraf, kolaj, video, slayt gösterisi, fontlar | ⏳ |
| M5 | Yerel Android uygulaması | ⏳ |
| M6 | Docker için tarayıcı arayüzü, CI | ⏳ |

Alfa şimdiye kadar Linux'ta ve Wine'da denendi; gerçek bir Windows bilgisayarında, gerçek bir MEGA hesabıyla ve gerçek bir telefonla henüz denenmedi.

## Bu sürümde olanlar

**Galeri ve görüntüleyici**
- Fotoğraflar zaman çizelgesi: kesin yerleşimli ızgara, tarih başlıkları, hızlı kaydırıcı; klavyeyle gezinme, Ctrl+tekerlek ile ızgara yoğunluğu
- Görüntüleyici: açılış/kapanış animasyonu, imlece göre yakınlaştırma, film şeridi, bilgi paneli (tarih, kamera, konum, dosya yolu), mini harita
- Video ve ses ffmpeg ile çözülür: her format oynar ve sarılabilir; hız, döngü, tam ekran, kareyi fotoğraf olarak kaydetme
- HEIC/AVIF Windows'ta da açılır (libheif paketin içinde)

**Seçim ve düzen**
- Tık, Ctrl, Shift, dikdörtgen seçim, Ctrl+A; seçim çubuğu ve sağ tık menüsü: favori, albüme ekle, zip indir, taşı/kopyala, yeniden adlandır (F2), sil
- Fotoğrafları albüme veya klasöre sürükle; bilgisayardan bırakılan dosyalar yüklenir
- Kitaplık, albümler, kütüphane sayfası (istatistikler, kapak, **Tümü | Klasörler**), klasör sayfaları, hariç tutma kuralları

**Keşfet ve harita**
- İstatistikler (yerel ve bulut boyutu ayrı), anılar ve geziler, yıl özeti, ülkeler ve şehirler, günün saatleri, renkler, biçimler, kameralar, kopyalar, takvim ısı haritası, "Beni şaşırt"
- Hikâye tarzı anı oynatıcı
- Uygulamanın kendi haritası: kümelenmiş küçük resimler, "bu alandaki fotoğraflar" paneli, standart / topoğrafik / uydu görünümü, çevrimdışı önbellek

<p align="center"><img src="docs/screenshots/map-2.0.jpg" width="90%" alt="2.0 harita: fotoğraflar konumlarına göre kümelenir, sağda o alandaki fotoğraflar"></p>

**Diğer**
- MEGA akış klasörleri ve MEGA hesabı, telefon eşleştirme (4 haneli kod), yükleme ve iş panelleri
- Bölümlere ayrılmış, Türkçe ve İngilizce aranabilir ayarlar; ayar yedeği
- Masaüstü: sistem tepsisi, tek pencere, oturum açılışında başlatma, bildirimler, görev çubuğunda ilerleme, güncelleme denetimi

**Henüz olmayanlar:** düzenleyiciler (fotoğraf, kolaj, video, slayt gösterisi oluşturucu, fontlar), yerel Android uygulaması, Docker'ın tarayıcı arayüzü. Anı oynatıcıda videolar henüz oynamıyor.

---

## Kurulum

### 🖥️ Windows

1. [v2.0.0-alpha.1](https://github.com/zearkreast/PeekAround/releases/tag/v2.0.0-alpha.1) sayfasından `PeekAround-2.0.0-alpha.1-win-x64.zip` dosyasını indir, bir klasöre çıkar ve `PeekAround.exe` dosyasını çalıştır. Kurulum gerekmez.
2. Paket imzasız: "Windows bilgisayarınızı korudu" uyarısında "Ek bilgi" → "Yine de çalıştır".
3. 1.x'teki verilerin (`%APPDATA%\PeekAround`) olduğu gibi okunur: kütüphaneler, favoriler, albümler, MEGA klasörleri, eşleşmiş telefonlar ve tercihler. 1.x ile 2.0'ı **aynı anda açma**; aynı veritabanını ve portu kullanırlar. İstersen önce bu klasörü yedekle.

Günlük dosyası: `%APPDATA%\PeekAround\logs\main.log`

### 📱 Telefon

Yerel Android uygulaması gelene kadar **1.3.2 APK** ile 2.0 masaüstüne bağlanabilirsin: masaüstünde Ayarlar → Ağ ve cihazlar → ağ paylaşımını aç (port `1313`). 2.0 masaüstüne tarayıcıdan bağlanılamaz, yalnızca uygulamayla.

### 🐳 Docker

2.0'ın tarayıcı arayüzü hazır olana kadar Docker için **1.x** kullan: kurulum adımları [`main` dalının README'sinde](https://github.com/zearkreast/PeekAround/tree/main#-docker-ağa-galeri-sunmak-için).

---

## Geliştirme

Gereksinimler: Rust 1.85+, ffmpeg (PATH'te). Linux'ta ayrıca `libheif` geliştirme paketi.

```bash
cargo build --release -p pa-desktop      # masaüstü uygulaması
cargo build --release -p pa-cli          # başsız sunucu (Docker / sunucu)
cargo test --workspace
```

Sunucu olmadan arayüzü denemek için: `pa-desktop --demo 10000` (10 bin öğelik sahte kütüphane). Ekran görüntüsü ve otomasyon için `PA_SCRIPT` ortam değişkeni (`wait 2000; nav explore; shot a.png; quit`).

### Yapı

```
crates/pa-core     Saf mantık ve API tipleri (1.x'teki packages/shared karşılığı)
crates/pa-server   HTTP/WebSocket sunucu: axum + tokio + SQLite
crates/pa-cli      peekaround: başsız sunucu (Docker)
crates/pa-desktop  Masaüstü uygulaması: Slint arayüz + gömülü sunucu, tepsi, bildirimler
crates/pa-ui       Slint arayüzü: tüm ekranlar ve görünüm modelleri
crates/pa-client   Tipli API istemcisi (HTTP + canlı olaylar)
crates/pa-media    Resim çözme/küçültme, thumbhash, ffmpeg sarmalayıcı
crates/pa-meta     EXIF / XMP, Google Takeout, video etiketleri
crates/pa-jobs     İş kuyruğu: video düzenleme, slayt dışa aktarma, canlı dönüştürme
crates/pa-mega     MEGA istemcisi: bağlantılar, şifreli parçalı indirme, hesap oturumu
crates/pa-remote   MEGA akış klasörleri: listeleme, küçük resim, önbellek, kota
crates/pa-link     Telefon bağlantısı: eşleştirme, cihaz klasörleri, aktarımlar
third_party/       Slint winit arka ucunun yamalı kopyası (0 Hz ekran düzeltmesi)
packages/          1.x TypeScript kodu (2.0 tamamlanana kadar başvuru için)
```

Teknik plan: [`docs/rust/PLAN.md`](docs/rust/PLAN.md)

## Atıflar

- Yer adları: [GeoNames](https://www.geonames.org/) (CC BY 4.0)
- Harita: © [OpenStreetMap](https://www.openstreetmap.org/copyright) katkıcıları; topoğrafik: © [OpenTopoMap](https://opentopomap.org) (CC-BY-SA); uydu: © Esri, Maxar, Earthstar Geographics
- Arayüz fontları Inter ve Bricolage Grotesque (SIL OFL), simgeler Lucide (ISC)

## Lisans

GNU GPL v3 — ayrıntılar `LICENSE` dosyasında.
