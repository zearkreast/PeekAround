<p align="center">
  <img src="brand/logo.svg" width="112" alt="PeekAround logo — a black-browed albatross looking left">
</p>

<h1 align="center">PeekAround</h1>
<p align="center"><b>Klasörlerin, en güzel haliyle.</b> · <i>Your folders, beautifully.</i></p>

PeekAround; bilgisayarındaki, sunucundaki ya da telefonundaki klasörleri modern, animasyonlu bir galeriye dönüştürür.
Fotoğraf ve videolarına göz at, izle, sürükle-bırak ile dosya/klasör/zip ekle, slayt gösterisi ve kolaj yap, fotoğraf ve videolarda basit düzenlemeler yap — **kendi fontlarınla** ve **orijinallere asla dokunmadan** (her düzenleme bir kopyaya kaydedilir).

Aynı uygulama üç şekilde çalışır:

| | Ne işe yarar | Paket |
|---|---|---|
| 🐳 **Docker / sunucu** | Ev sunucuna/NAS'a kur, galeriyi tüm ağa sun (tarayıcıdan, telefondan) | `docker compose up -d` |
| 🖥️ **Masaüstü uygulaması** | Kişisel galeri; istersen tek tıkla ağda paylaş | Windows `.exe`, Arch Linux `.pacman` / PKGBUILD, AppImage, `.deb` |
| 📱 **Android** | Telefonun kendi klasörlerini gez **veya** bir PeekAround sunucusuna bağlan | `.apk` |

---

## Özellikler

**Galeri**
- Kütüphane = bir veya birden çok klasör; klasör ağacı, zaman çizelgesi (ay başlıkları + tarih çubuğu), favoriler, arama, sıralama/filtre
- Sanallaştırılmış *justified* ızgara (on binlerce dosya akıcı), kare ızgara, boyut kaydırıcısı
- Küçük resimden tam ekrana **paylaşılan öğe geçişi**, zoom/pan/pinch, kaydırarak gezinme, film şeridi, EXIF bilgi paneli
- Video: özel oynatıcı (önizlemeli zaman çubuğu, hız, döngü, PiP, kareyi fotoğraf olarak kaydet); tarayıcının oynatamadığı formatlar (MKV/HEVC/AVI…) sunucuda anlık dönüştürülür
- Videoların üzerine gelince sessiz önizleme, thumbhash ile anında bulanık yer tutucu
- Seçim: tık, Shift-aralık, Ctrl, **kement (dikdörtgen sürükleme)**; toplu favori/indir/taşı/kopyala/sil
- Diskteki değişiklikler **canlı** yansır (dosya izleme + WebSocket)

**Ekleme**
- Uygulamanın üzerine **dosya, klasör veya zip sürükle-bırak** (kenar çubuğundaki/kart üzerindeki klasöre bırakma da olur)
- Zip için istem: **Kalıcı olarak çıkar** veya **Geçici olarak aç** + **“Seçimimi hatırla”** kutucuğu (Ayarlar’dan değiştirilebilir). Geçici oturumlar kendiliğinden silinir, istersen “Kalıcı yap”.
- Kütüphanedeki zip’lere “Göz at” ile geçici bakma; Türkçe Windows zip’lerindeki (CP857) dosya adları doğru çözülür
- Masaüstünde bırakılan klasör: **kütüphaneye bağla** (kopyalamadan) veya **buraya kopyala**
- Android’de **Paylaş → PeekAround**

**Düzenleme (her zaman kopyaya: `ad_edit.jpg`, `ad_edit-2.jpg` …)**
- Fotoğraf editörü: pozlama, kontrast, sıcaklık, gölge/vurgu, vinyet, gren… (WebGL), filtre ön ayarları, kırp/döndür/çevir/düzleştir, **özel fontlu metin katmanları**, geri/ileri al, önce/sonra
- Kolaj: şablonlar, oranlar (1:1, 4:5, 9:16…), boşluk/köşe/arka plan, hücre içi kaydırma/yakınlaştırma, metin
- Video editörü: kırpma (trim), crop, döndür/çevir, hız, ses, renk ayarları, zamanlı metin katmanları → MP4/WebM/GIF
- Slayt gösterisi: tarayıcıda animasyonlu geçişlerle oynat **veya** müzikli MP4 olarak dışa aktar (Ken Burns, geçiş efektleri)
- **Fontlar**: TTF/OTF/WOFF/WOFF2 yükle; tüm editörlerde canlı önizlemeli font seçici; Türkçe karakterli hazır fontlar

**Diğer**
- Türkçe / İngilizce arayüz, koyu/açık tema, vurgu rengi, animasyon seviyesi (tam/azaltılmış/kapalı)
- PWA: telefondan “Ana ekrana ekle”
- Ağdaki sunucular mDNS ile otomatik bulunur (Android uygulaması), ayarlarda bağlantı adresi + QR kod

---

## Kurulum

### 🐳 Docker (ağa galeri sunmak için)

```bash
git clone https://github.com/<kullanici>/peekaround.git
cd peekaround
```

`docker/docker-compose.yml` içindeki klasör yollarını kendine göre düzenle (her `/media/<Ad>` bir kütüphane olur), sonra:

```bash
docker compose -f docker/docker-compose.yml up -d --build
```

Tarayıcıdan `http://<sunucu-ip>:8080` adresine git.

Hazır imaj (GitHub Actions ile `ghcr.io`'ya gönderilir, amd64 + arm64/Raspberry Pi):

```bash
docker run -d --name peekaround -p 8080:8080 -e PUID=1000 -e PGID=1000 -v /yol/Fotoğraflar:/media/Fotoğraflar -v ./peekaround-data:/data ghcr.io/<kullanici>/peekaround:latest
```

| Değişken | Varsayılan | Açıklama |
|---|---|---|
| `PEEK_ROOTS` | `/media` | İlk açılışta kütüphane yapılacak klasör(ler); `Ad=/yol` biçimi, virgülle ayrılır |
| `PEEK_SPLIT_ROOTS` | `true` | Kökün her alt klasörü ayrı kütüphane olsun |
| `PUID` / `PGID` | `0` | Yazılan dosyaların sahibi (`id -u` / `id -g`) |
| `PEEK_WATCH_POLL` | `false` | Ağ paylaşımlarında (NFS/SMB) dosya değişikliklerini yoklayarak izle |
| `PEEK_READONLY` | `false` | Yükleme/düzenleme/silmeyi kapat |
| `PEEK_BROWSE_ROOTS` | `/media` | “Klasör ekle” gezgininin gösterebileceği dizinler |
| `PEEK_PORT` | `8080` | Port |
| `TZ` | – | Saat dilimi (ör. `Europe/Istanbul`) |

> Kimlik doğrulama yoktur: PeekAround’u yalnızca güvendiğin ağlarda çalıştır ya da önüne bir ters vekil (reverse proxy) + şifre koy.

### 🖥️ Masaüstü

- **Windows:** Releases’tan `PeekAround-Setup-x.y.z.exe` (kurulum) veya `PeekAround-x.y.z-portable.exe`
- **Arch Linux:** `sudo pacman -U PeekAround-x.y.z-x64.pacman` ya da `packaging/arch/PKGBUILD` ile `makepkg -si`
- **Diğer Linux:** `PeekAround-x.y.z-x86_64.AppImage` (çalıştırılabilir yap ve aç) veya `.deb`

İlk açılışta Resimler ve Videolar klasörlerin kütüphane olarak eklenir. Ayarlar → Ağ’dan **“Galeriyi yerel ağda paylaş”** ile aynı uygulama ağdaki diğer cihazlara da galeri sunar.

### 📱 Android

Releases’tan `PeekAround-*.apk` indir ve yükle (bilinmeyen kaynaklara izin ver). İlk açılışta seç:
- **Bu telefondaki fotoğraflar** — “Tüm dosyalara erişim” iznini ver; Kamera/Resimler/Filmler/İndirilenler/WhatsApp kütüphane olarak eklenir. Düzenlemeler, videolar ve slaytlar telefonda (Media3, donanım kodlayıcı) işlenir.
- **Bir PeekAround sunucusuna bağlan** — ağdaki sunucular otomatik bulunur ya da adresi yaz.

Bağlantıyı daha sonra Ayarlar → Bağlantı’dan değiştirebilirsin.

---

## Geliştirme

Gereksinimler: Node.js ≥ 22.13 (24 önerilir), ffmpeg + ffprobe (PATH’te).

```bash
npm install
npm run fixtures      # örnek medya: fixtures/library
npm run dev           # API :8787 + arayüz :5173 (http://localhost:5173)
```

```bash
npm run typecheck
npm test
npm run build                       # web + sunucu
node packages/server/dist/cli.js --root ~/Resimler   # tek komutla sunucu
```

Masaüstü:

```bash
npm run build -w @peekaround/web
node packages/desktop/scripts/stage.mjs                 # stage/ klasörü
npx electron packages/desktop/stage                    # çalıştır
node packages/desktop/scripts/dist.mjs --linux          # AppImage + pacman + deb (bsdtar gerekir)
node packages/desktop/scripts/stage.mjs --platform=win32 --arch=x64 && node packages/desktop/scripts/dist.mjs --win
```

Android (JDK 21 + Android SDK, `ANDROID_HOME` ayarlı):

```bash
npm run build -w @peekaround/web
cd packages/android && npx cap sync android && cd android && ./gradlew assembleDebug
```

İkonlar `brand/logo.svg`’den üretilir: `node scripts/make-icons.mjs`

### Yapı

```
packages/shared    Ortak TypeScript API sözleşmesi
packages/server    Fastify 5 sunucu (Docker + masaüstü içinde aynı kod): tarama, izleme, küçük resim (sharp), ffmpeg işleri, zip, fontlar
packages/web       React 19 + Vite + Tailwind 4 + Motion arayüz (tüm platformlar)
packages/desktop   Electron kabuğu (gömülü sunucu, yerel klasör seçici, ağda paylaş)
packages/android   Capacitor 8 + Kotlin eklentisi (yerel medya sunucusu, Media3, mDNS, paylaşım)
docker/            Dockerfile + compose
brand/             Logo (albatros)
```

### GitHub’a gönderince

`.github/workflows/build.yml` her push’ta test eder, Windows/Linux paketlerini, Android APK’yı ve Docker imajını derler (Actions → Artifacts).
`v1.0.0` gibi bir etiket gönderdiğinde hepsini bir **Release**’e ekler:

```bash
git tag v1.0.0
git push origin v1.0.0
```

İmzalı (release) APK için repo Secrets: `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD`. Yoksa debug imzalı APK üretilir.

## Lisans

MIT. Hazır fontlar SIL Open Font License ile dağıtılır (@fontsource).
