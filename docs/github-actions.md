# GitHub Actions: otomatik derleme ve gerekli token'lar

`.github/workflows/build.yml` şunları yapar:

| Ne zaman | Ne yapar |
|---|---|
| `main`'e her push ve her pull request | Rust testleri (`cargo test --workspace`) |
| `v2.0.0` gibi bir etiket | Testler, Windows (`Setup.exe` + taşınabilir zip), Linux (`tar.gz`), Android APK, ardından **GitHub Release**. Etikette `-` varsa (`v2.1.0-beta.1`) ön sürüm olarak yayınlanır |
| `build-deneme` gibi bir etiket (`build-` ile başlayan) | Aynı paketler, Release olmadan; paketler çalıştırmanın "Artifacts" bölümüne eklenir |
| Actions sekmesi → Build → **Run workflow** | Etiketsiz deneme derlemesi; `release_tag` yazılırsa paketler o mevcut sürüme yüklenir |

Docker imajı bu iş akışında derlenmiyor; `ghcr.io/<kullanıcı>/peekaround` altındaki son imaj 1.3.2.

## 1. Hangi token gerekli?

**Workflow için hiçbir kişisel token gerekmez.** GitHub her çalıştırmaya otomatik olarak `GITHUB_TOKEN` verir. Release oluşturma bununla yapılır; izinler workflow dosyasında yazılı.

Bir kez kontrol et: repo → **Settings → Actions → General**

- *Actions permissions*: "Allow all actions and reusable workflows" seçili olmalı.
- *Workflow permissions*: varsayılan "Read repository contents" kalabilir. Yazma izni gereken iş (release) bu izni kendisi istiyor.

## 2. Bilgisayardan GitHub'a push için token (gerekirse)

Git, HTTPS ile push ederken şifre yerine token ister. Windows'ta "Git Credential Manager" tarayıcıdan giriş yaptırıyorsa token oluşturmana gerek yok.

Token gerekiyorsa:

1. GitHub'da sağ üstte profil resmi → **Settings → Developer settings → Personal access tokens → Fine-grained tokens → Generate new token**.
2. Formu şöyle doldur:
   - **Token name:** `PeekAround push`
   - **Expiration:** örneğin 90 gün
   - **Repository access:** *Only select repositories* → `PeakAround`
   - **Permissions → Repository permissions:**
     - **Contents:** Read and write
     - **Workflows:** Read and write. Bu olmadan `.github/workflows/` altındaki dosyaları push edemezsin; GitHub "refusing to allow a Personal Access Token to create or update workflow" hatası verir.
     - **Metadata:** Read-only (otomatik gelir)
3. **Generate token**'a bas ve token'ı kopyala. Token bir daha gösterilmez.
4. İlk `git push`'ta şunları gir:
   - kullanıcı adı: GitHub kullanıcı adın
   - şifre: token

Klasik token kullanıyorsan (**Tokens (classic)**) `repo` ve `workflow` kutularını işaretle.

## 3. İmzalı Android APK (önerilir, ücretsiz)

Android'de her APK imzalıdır. Önemli olan imzanın **hep aynı anahtarla** atılması: telefon, uygulamanın yeni sürümünü ancak aynı anahtarla imzalanmışsa eskisinin üzerine kurar.

Secret eklemezsen workflow **debug** imzalı bir APK üretir. GitHub'ın makinelerinde bu debug anahtarı her çalıştırmada yeniden oluşur. Sonuç: her sürümün imzası farklı olur ve telefonda "uygulama yüklenmedi / paket çakışıyor" hatası alırsın. Bu durumda eskisini kaldırıp yeniden kurmak gerekir, bu da uygulamanın ayarlarını ve eşleştirmesini siler.

Bunu önlemek için kendi anahtarını bir kez oluştur. Ücretsizdir, sertifika satın alman gerekmez:

1. **Anahtar oluştur.** `keytool` Android Studio veya JDK ile gelir. Örneğin Windows'ta `C:\Program Files\Android\Android Studio\jbr\bin\keytool.exe`.

```bash
keytool -genkeypair -v -keystore peekaround-release.keystore -alias peekaround -keyalg RSA -keysize 4096 -validity 10000
```

   Sorulan şifreyi ve bilgileri gir. **Bu dosyayı ve şifreyi kaybetme.** Anahtar değişirse telefondaki uygulama güncellenemez; kaldırıp yeniden kurman gerekir.

2. **Dosyayı base64 metne çevir.**

   Windows PowerShell'de (metin panoya kopyalanır):

```powershell
[Convert]::ToBase64String([IO.File]::ReadAllBytes("peekaround-release.keystore")) | Set-Clipboard
```

   Linux veya macOS'ta:

```bash
base64 -w0 peekaround-release.keystore
```

3. **Secret'ları ekle.** Repo → **Settings → Secrets and variables → Actions → New repository secret**:

| Ad | Değer |
|---|---|
| `ANDROID_KEYSTORE_BASE64` | 2. adımdaki base64 metin |
| `ANDROID_KEYSTORE_PASSWORD` | keystore şifresi |
| `ANDROID_KEY_ALIAS` | `peekaround` (keytool'da `-alias` ile verdiğin ad) |
| `ANDROID_KEY_PASSWORD` | anahtar şifresi (keytool ayrı sormadıysa keystore şifresiyle aynı) |

4. Sonraki etiketten itibaren Release'te `PeekAround-<sürüm>-android-release.apk` olur. Telefonda debug APK kuruluysa, imza değiştiği için bir kez kaldırıp yeniden kur.

## 4. Windows imzası (isteğe bağlı, genelde ücretli)

Secret yoksa Windows `.exe` dosyaları **imzasız** çıkar. Uygulama çalışır, ama ilk açılışta SmartScreen "Windows bilgisayarınızı korudu" uyarısı verir ("Ek bilgi" → "Yine de çalıştır").

Uyarıyı kaldırmanın yolları:

| Yol | Maliyet | Not |
|---|---|---|
| **SignPath Foundation** (signpath.org) | Açık kaynak projelere ücretsiz | Başvuru ve onay gerekir. GPL lisanslı, herkese açık repo uygun. İmzalama SignPath'in kendi GitHub Action'ı ile yapılır; onaylanırsan workflow'a o adım eklenir. |
| **Azure Trusted Signing** (Microsoft) | Aylık birkaç dolar | Bireysel kimlik doğrulaması her ülkede açık değil; Türkiye'den bireysel kayıt mümkün olmayabilir. |
| **OV/EV kod imzalama sertifikası** (Sectigo, DigiCert, Certum…) | Yılda yaklaşık 100–400 $ | Yeni sertifikalarda bile SmartScreen uyarısı, uygulama yeterince indirilene kadar bir süre görünebilir. |

İş akışında şu an imzalama adımı yok; sertifikan olursa Windows işine eklenmesi gerekir.

## 5. Sürüm yayınlama

```bash
git tag v2.0.0
```

```bash
git push origin v2.0.0
```

Actions sekmesinde **Build** çalışır. Bitince **Releases** sayfasında paketler görünür.

- Etiketin sürümü `Cargo.toml` içindeki `version` ile aynı olmalı (`v2.0.0` ↔ `2.0.0`); değilse Release işi durur.
- Bir etiketi baştan derletmek istersen:

```bash
git push origin :refs/tags/v2.0.0
```

```bash
git tag -f v2.0.0
```

```bash
git push origin v2.0.0
```

## Sorun giderme
- **Testler düşüyor:** Actions'ta kırmızı adıma tıkla, `FAILED` veya `panicked` satırını bul.
- **Release oluşmadı:** Release yalnızca `v` ile başlayan etiketlerde oluşur. Ayrıca testler ile Windows, Linux ve Android işlerinin hepsi başarılı olmalı.
