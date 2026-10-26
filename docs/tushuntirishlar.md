# spacewm — har bosqich bo'yicha tushuntirishlar

## 0-bosqich

**calloop nima va nega kompozitor event loop atrofida quriladi?**
calloop — "reaktor": jarayon hech narsa qilmay uxlaydi (epoll), biror fd tayyor bo'lganda
(klient so'rov yubordi, sichqoncha qimirladi, vblank keldi, taymer tugadi, signal keldi)
uyg'onib, o'sha manbaga bog'langan callback'ni chaqiradi. Kompozitorda deyarli hamma ish
qisqa va bir-biriga bog'liq: bitta klientning commit'i keyingi kadrni chizishga, kadr esa
frame callback'larga olib keladi. Threadlar bilan bularni lock'lar bilan himoyalash kerak
bo'lardi (deadlock, race). Bitta thread + event loop: hamma narsa navbatma-navbat, lock yo'q.
Og'ir ish (PNG kodlash, chunk meshing) alohida threadga beriladi va natija kanal orqali
qaytadi.

**Nega hamma holat bitta `SpaceState` da?** smithay handler trait'lari (CompositorHandler,
SeatHandler, ...) bitta turga implement qilinadi va calloop har callback'ga `&mut SpaceState`
beradi. Shunda istalgan callback istalgan holatga to'g'ridan-to'g'ri yetadi —
`Rc<RefCell<..>>` yoki `Mutex` kerak emas.

**anyhow vs thiserror:** modullar `SpaceError` (thiserror) qaytaradi — bu aniq tipli enum,
chaqiruvchi `match` qila oladi. `main.rs` esa `anyhow` ishlatadi va har xatoga
`.context("...")` qo'shadi, shuning uchun yakuniy xabar "failed to start winit backend:
EGL ..." kabi butun zanjirni ko'rsatadi.

## 1-bosqich

**Bitta kadrning hayoti:** klient o'z bufferiga chizadi → `wl_surface.attach(buffer)` +
`damage` → `commit` → bizning `CompositorHandler::commit` (`on_commit_buffer_handler`
bufferni renderer uchun import qiladi, oyna geometriyasi yangilanadi) → biz qayta chizishni
rejalashtiramiz → render (damage tracker faqat o'zgargan joyni chizadi) → kadr ekranga
chiqadi → `send_frame` bilan frame callback yuboramiz → klient keyingi kadrni chizadi.
Frame callback bo'lmasa klient bitta kadrdan keyin "qotib" qoladi.

**wl_surface / xdg_surface / xdg_toplevel / buffer:** `wl_surface` — piksellar
to'rtburchagi, o'zi hech qanday ma'noga ega emas. `xdg_surface` unga "shell" rolini beradi
(configure/ack mexanizmi), `xdg_toplevel` esa uni ilova oynasiga aylantiradi (title,
app_id, o'lcham, maximized holat). `buffer` — surface'ga biriktiriladigan haqiqiy piksellar
(shm: CPU xotirasi, dmabuf: GPU xotirasi).

**Serial nima?** Har input hodisasiga (tugma bosildi, klavish bosildi) kompozitor noyob
raqam beradi. Klient keyinchalik "menyu och", "oynani surish", "clipboard'ni o'rnat" desa,
o'sha serialni ko'rsatishi shart. Biz serial haqiqatan yaqinda shu klientga berilganini
tekshiramiz — shunda fon ilovasi o'zicha popup grab olib klaviaturani o'g'irlay olmaydi.

## 2-bosqich

**Model, view, projection:** `model` ob'ekt koordinatalarini dunyo koordinatalariga,
`view` dunyoni kamera atrofiga (kamera koordinata boshida, −Z ga qaraydi),
`projection` esa perspektiva bilan clip space'ga o'tkazadi. Misol: kamera (0, 1.7, 3) da,
to'g'ri oldinga qaraydi (pitch 0). Dunyo nuqtasi (0, 1.7, −7): model = I → (0, 1.7, −7);
view kamerani boshga suradi → (0, 0, −10); projection (fov 75°, near 0.05) → clip w = 10,
x = y = 0 → NDC (0, 0, ≈0.99): ekran markazida, chuqurlik deyarli uzoq tomonda.

**Depth buffer:** har piksel uchun shu paytgacha chizilgan eng yaqin narsaning
chuqurligini saqlaydi. Yangi uchburchak piksel uchun faqat yaqinroq bo'lsa yoziladi.
Busiz chizish tartibi g'olib bo'ladi: keyin chizilgan uzoq tepalik oldingi devorning ustiga
chiqadi.

**Nega smithay uchun GL holatini tiklaymiz?** smithay renderer GL standart holatda
(depth test o'chiq, culling o'chiq, blending yoqiq, bizning VAO/FBO bog'lanmagan) deb
hisoblaydi. Biz depth/cull yoqib, o'z VAO'imizni bog'lab qoldirsak, smithay keyingi
chizishda oynalarni noto'g'ri kesib tashlaydi yoki bizning VAO'imizni buzadi.

**Arxitektura qarori:** 3D sahna o'zimizning offscreen framebuffer'imizga (MSAA rang +
depth) chiziladi, keyin smithay uni oddiy textura sifatida kursor bilan birga ekranga
chiqaradi. Shuning uchun depth buffer har backend'da bor, winit'ning ag'darilgan surface'i
va kelajakdagi aylantirilgan monitorlarni smithay o'zi to'g'rilaydi, damage tracking ishlaydi:
kamera qimirlamasa sahna umuman qayta chizilmaydi.

## 3-bosqich

**shm va dmabuf bufferlar qanday qilib GlesTexture bo'ladi?**
`shm`: klient CPU xotirasida chizadi va memfd'ni ulashadi. smithay surface'ni birinchi
chizishda piksellarni `glTexImage2D`/`glTexSubImage2D` bilan GPU texturasiga yuklaydi,
keyingi commit'larda faqat damage qilingan qismlarni yuklaydi. `dmabuf`: klient allaqachon
GPU xotirasida chizgan. smithay uni EGLImage sifatida "o'raydi" va textura shu xotiraning
o'ziga qaraydi, nusxa ko'chirilmaydi. Ikkala holatda textura surface'ning
`RendererSurfaceState`ida keshlanadi.

**Nega har oyna uchun alohida offscreen textura?** Oyna bitta surface emas, daraxt:
toplevel + subsurface'lar (video, embed qilingan EGL) + popuplar. Ularning har birida o'z
texturasi, siljishi, viewporter crop/scale va buffer transform bor. smithay'ning oddiy 2D
renderer'i bularning hammasini biladi, shuning uchun biz daraxtni bitta panel texturasiga
shu renderer bilan chizdiramiz, 3D shader esa faqat shu bitta texturani namuna qiladi.
Qo'shimcha foyda: mipmap'lar (uzoqdagi matn "jimirlamaydi") va popuplar bbox ichida
kesilmaydi.

**Damage tracking bu yerda nimani tejaydi?** Har panelning o'z damage tracker'i bor va
textura avvalgi tarkibini saqlaydi (age = 1). Klient commit qilmasa, panel umuman qayta
chizilmaydi. Commit qilsa, faqat o'zgargan to'rtburchaklar qayta chiziladi. Bo'sh terminal
sekundiga 0 marta qayta chiziladi (test buni tekshiradi). Sahna ham faqat biror panel
texturasi yoki kamera o'zgarganda qayta chiziladi.

**Spec'dan ongli farq:** yangi panel 2 m emas, "1:1 piksel" masofasida paydo bo'ladi:
`d = H / (2·800·tan(37.5°))`, 1080p'da ≈0.88 m. Agar panel ekranning 85 foizidan
kattaroq bo'lib qolsa, biroz uzoqroqqa qo'yiladi. 2 m'da 1000 px'lik oyna 1080p ekranda
~440 px bo'lib, matn o'qib bo'lmas edi, bu esa "matn aniq ko'rinsin" talabiga zid.

## 4-bosqich

**Nur va tekislik kesishishi (raqamli misol):** nur `o + t·d`, tekislik esa `p0` nuqta va `n`
normal bilan beriladi. Tekislikdagi har bir `p` nuqta uchun `dot(p − p0, n) = 0`. `p` o'rniga
nurni qo'ysak, `t = dot(p0 − o, n) / dot(d, n)` chiqadi. Misol: kamera `o = (0, 1.7, 0)`,
qarash yo'nalishi `d = (0, 0, −1)`. Panel `p0 = (0, 1.7, −2)` da, `n = (0, 0, 1)`. Unda
`t = dot((0,0,−2), n) / dot(d, n) = −2 / −1 = 2`, ya'ni nur panelga 2 metrda tegadi. Agar
`dot(d, n) ≈ 0` bo'lsa, nur tekislikka parallel va kesishish yo'q. `t < 0` bo'lsa, panel
orqada qoladi.

**3D nuqta qanday qilib Wayland koordinatasiga aylanadi?** Tegish nuqtasini panelning
teskari model matritsasiga ko'paytiramiz. Shunda panel −0.5..0.5 o'lchamli birlik kvadratga
aylanadi. `px = (x + 0.5)·kenglik`, `py = (0.5 − y)·balandlik` (Y teskari: Wayland'da y
pastga o'sadi). Natija oyna geometriyasidagi piksel, klient aynan shuni kutadi. Test buni
butun zanjir orqali tekshiradi: ekranga bosiladi → nur → panel → klient (weston-clickdot)
belgi chizadi → panel texturasi → 3D → ekran. Belgi bosilgan joydan ±2 px ichida chiqadi.

**Nega Wayland'da fokus aniq beriladi?** X11'da istalgan dastur o'zini fokusga qo'yishi,
boshqa oynaning tugmalarini "tinglashi" yoki ularni soxta yuborishi mumkin edi.
Wayland'da klaviatura fokusini faqat kompozitor beradi va klient faqat o'zi fokusda
bo'lganda tugmalarni oladi. Shuning uchun spacewm'da fon dasturi yangi oyna ochib fokusni
o'g'irlay olmaydi. Fokusni faqat foydalanuvchi o'zi hozirgina ishga tushirgan dasturning
oynasi oladi (5 soniya ichida), qolganlarini bosish kerak.

**Nested rejimda sichqoncha:** smithay'ning tayyor winit backend'i xom (relative)
harakatni uzatmaydi. Shu sababli o'z backend'imizni yozdik: host pointer qulflanadi
(pointer-constraints) va relative-pointer'dan cheksiz burilish olinadi. Ctrl+Alt+G
sichqonchani host'ga qaytaradi, oynaga bosish uni yana qulflaydi.

## 5-bosqich

**Nega chunklar va face culling? Qancha uchburchak tejaladi?** Olam 16×16×16 bloklik
chunklarga bo'lingan. Bitta blok o'zgarsa, faqat uning chunki (va chegarada bo'lsa qo'shnisi)
qayta mesh qilinadi. Ko'rinmaydigan chunklar (orqada yoki 8 chunkdan uzoqda) umuman
chizilmaydi. Face culling: blokning yuzi faqat yonidagi blok shaffof bo'lsa chiziladi.
Misol: to'liq tosh chunk sodda usulda 4096 blok × 12 uchburchak = 49 152 uchburchak bo'ladi.
Cullingdan keyin faqat tashqi qobiq qoladi: 6 × 256 yuz × 2 = 3 072 uchburchak (94% kam).
Yer ostidagi tosh qatlamlarda deyarli hech narsa chizilmaydi.

**Eksa bo'yicha ajratilgan AABB to'qnashuvi:** o'yinchi 0.6×1.8×0.6 m quti. Har fizika
qadamida (sekundiga 120 marta) avval faqat Y bo'yicha siljiydi: kesishgan qattiq blok
bo'lsa, quti uning yuzasigacha qaytariladi va "yerda" bayrog'i qo'yiladi. Keyin xuddi
shunday X, keyin Z bo'yicha. Har eksa alohida hal qilingani uchun devorga tegsangiz
yopishib qolmaysiz, uning bo'ylab sirpanasiz, burchakda esa blok ichiga kirib qolmaysiz.
Qat'iy qadam (1/120 s) tufayli sakrash balandligi FPS'ga bog'liq emas.

**Nega GL asosiy threadda, meshing esa worker'da?** OpenGL konteksti bitta threadga
bog'langan: u qaysi threadda "current" bo'lsa, GL chaqiruvlari faqat o'sha yerda ishlaydi.
Boshqa threaddan chaqirish aniqlanmagan xatti-harakat yoki crash beradi. Meshing esa sof
hisoblash: chunk nusxasidan (qo'shnilardan bir blok chegarasi bilan) uchlar ro'yxatini
yasaydi, GL'ga tegmaydi. Shuning uchun u worker threadda bajariladi. Tayyor mesh calloop
kanali orqali qaytadi, asosiy thread esa uni bir kadrda ko'pi bilan 4 tadan GPU'ga yuklaydi.
Shu sababli kadr hech qachon "qotmaydi" (eng uzun kadr ~2 ms).

**Ongli qarorlar:** barglar shaffof o'tishda emas, alpha-test (cutout) bilan chiziladi.
Bu chuqurlik buferini to'g'ri saqlaydi va standart usul. Bloklar uchun atlas emas,
textura massivi ishlatiladi, shunda mipmap'lar qo'shni plitkalarga "oqmaydi". Paydo bo'lish
joyi platforma atrofida tekislangan o'tloq bo'ladi va kamera eng ochiq tomonga qaraydi.

## 6-bosqich

**DRM master, KMS, CRTC, connector, plane.** DRM — kernelning GPU interfeysi
(`/dev/dri/card1`). KMS (kernel mode setting) — uning displey qismi: qaysi monitorga
qanday rezolyutsiya va qaysi buferni ko'rsatish. *Connector* — jismoniy port (eDP-1,
HDMI-A-1); unda monitor, uning EDID'i (ishlab chiqaruvchi, model, mm o'lchami) va
rejimlari bor. *CRTC* — "skanlovchi": framebuffer'ni o'qib, rejim taktida connector'ga
uzatadi (sizda 4 ta, ya'ni bir vaqtda 4 ta monitor). *Plane* — CRTC apparatda
qo'shadigan qatlam: primary (butun kadr), cursor (sichqoncha), overlay. *DRM master* —
KMS'ni o'zgartirishga ruxsati bor yagona jarayon; aks holda ikki dastur bir ekranga
navbatma-navbat yozib, tartibsizlik bo'lardi.

**Nega vblank bo'yicha chizamiz, page flip nima.** Monitor ekranni yuqoridan pastga
sekundiga 60 marta o'qiydi; kadrlar orasidagi qisqa pauza — vertical blank (vblank).
Biz yangi kadrni alohida buferga chizamiz va `queue_frame` bilan "keyingi vblank'da shu
buferga o'tkaz" deymiz — bu *page flip*. Almashinuv atomar, shuning uchun yarim chizilgan
kadr hech qachon ko'rinmaydi (tearing yo'q). Kernel flip bo'lganini VBlank voqeasi bilan
aytadi: aynan o'sha payt keyingi kadrni boshlash va mijozlarga frame callback yuborish
vaqti. Shunda 60 Hz'dan ortiq behuda chizmaymiz, hech narsa o'zgarmasa umuman
chizmaymiz (taymer bilan taxminiy vblank), va kechikish bir kadrdan oshmaydi.

**libseat nima qiladi, nega root kerak emas.** GPU va /dev/input qurilmalarini faqat
root ocha oladi. libseat (logind yoki seatd orqali) bizning login sessiyamiz nomidan
ularni ochib, fd'ni bizga beradi va DRM master'ni topshiradi. Boshqa VT'ga o'tilganda
sessiya qurilmalarni bizdan *tortib oladi* (PauseSession) — shuning uchun Ctrl+Alt+F1
ishlaydi va qaytganda (ActivateSession) to'liq modeset qilib, hammasini qayta chizamiz.
Root kompozitor xavfli: undagi har bir xato (bizda 10 000+ qator kod) va unga ulangan
har bir mijoz butun tizimga yo'l topishi mumkin. Shuning uchun spacewm root'da ishga
tushishdan bosh tortadi.

**Sinov (haqiqiy eDP-1 da).** `tests/drm/` skriptlari spacewm'ni tty8 da haqiqiy logind
sessiya sifatida ishga tushiradi: 60 s ishladi (80 screenshot), VT'ga chiqib-qaytganda
rasm, oynalar va input qaytdi, Ctrl+Alt+F1, Mod+Enter va Ctrl+Alt+Backspace ishladi,
2D rejim ham. Topilgan xato: libinput qurilmalarni ichki navbatga qo'yib fd'ni
uyg'otmasdi — qurilmalar 3.5 s kechikib sozlanardi; endi navbatni darhol o'qiymiz.
Sinalmaganlar: ikkinchi monitorni ulash (sizda yo'q edi), Firefox dmabuf, SDDM orqali kirish.

## 7.1-bosqich: XWayland

**XWayland nima.** Haqiqiy X server, lekin o'zi Wayland mijozi: har bir X11 oynasini
o'zining wl_surface'iga chizadi. Bizning X11 oyna menejerimiz (smithay `X11Wm`, oddiy X
ulanishi) qaysi X oynasi qaysi surface ekanini aytadi — shunda X11 oynalar ham oddiy
`Window` va panelga aylanadi. Kechiktirilgan ishga tushirish: spacewm `/tmp/.X11-unix/X1`
soketini o'zi ochadi; birinchi X dastur ulanganda Xwayland'ni shu soketlar bilan
(`-listenfd`) ishga tushiradi, kutib turgan ulanish yo'qolmaydi. Ishga tushgach to'xtatilmaydi:
Qt/Electron ilovalari oynasiz ham X ulanishini ushlab turadi (avval "X oynasi yo'q bo'lsa
to'xtatish" bor edi — u Telegram'ni muzlatib qo'ydi, olib tashlandi). Crash bo'lsa keyingi
X dastur yangi serverni ishga tushiradi.

**Nega override-redirect oynalar alohida.** X11'da menyu, tooltip, combo ro'yxati WM'dan
ruxsat so'ramaydi: o'zi *root* (global) koordinatada paydo bo'ladi va sichqonchani ushlaydi.
Ilova bu joyni "mening oynam qayerda" degan bilimidan hisoblaydi. 3D'da "ekrandagi joy"
yo'q, shuning uchun har X oynasiga 2D'da qat'iy joy beramiz va aynan shunga configure
qilamiz: menyu joyi − oyna joyi = menyuning panel ichidagi joyi. Ular alohida panel emas,
ota-panel teksturasiga chiziladi va sichqoncha ular ustida bo'lsa input ularga boradi.

**Topilgan xatolar:** (1) signalfd faqat signal *hamma* thread'da bloklansa ishlaydi —
mesher thread'i bloklamagani uchun SIGTERM spacewm'ni tozalamasdan o'ldirar edi; endi
main() boshida bloklanadi. (2) smithay X clipboard egasini o'zgartirgach flush qilmaydi.

## 7.2-bosqich: config fayl

`~/.config/spacewm/config.toml` birinchi ishga tushishda barcha kalitlar va izohlar bilan
yoziladi (unit test u `Config::default()` bilan bir xil ekanini tekshiradi). serde
`deny_unknown_fields` har noma'lum kalitni xato qiladi; xabarda qator:ustun, kalit va eng
yaqin to'g'ri nom bor (Levenshtein masofasi). Qiymatlar alohida tekshiriladi (fov 30-120,
NaN rad etiladi, xkb layout haqiqatan kompilyatsiya qilib ko'riladi). Fayl inotify bilan
kuzatiladi — fayl emas, PAPKA, chunki muharrirlar yangi fayl yozib rename qiladi. O'zgarish
150 ms kutib (bir saqlash bir necha hodisa beradi) jonli qo'llanadi; xato bo'lsa eski
sozlamalar qoladi va ekranda qizil panel chiqadi. Mod+Escape config'dan qat'i nazar doim
ishlaydi — xato tahrir sizni oyna ichida qamab qo'ymasligi uchun.

## 7.3-bosqich: IPC va spacectl

Soket `$XDG_RUNTIME_DIR` (0700) ichida 0600 bilan yaratiladi va har ulanishda SO_PEERCRED
bilan uid tekshiriladi: root ham rad etiladi (sinab ko'rildi). So'rov ≤64 KiB, ≤16 ulanish,
o'qilmagan javob ≤1 MiB; hamma I/O non-blocking — osilib qolgan mijoz kompozitorni
to'xtatmaydi. `spawn` buyruqni shell'siz ishga tushiradi (so'zlarga bo'linadi, lekin
`|`, `>`, `$(...)` ishlamaydi); `--shell` faqat config'da `ipc_allow_shell = true`
bo'lsa. Screenshot faqat `$HOME` ichiga, symlink orqali emas (O_NOFOLLOW), mavjud faylni
faqat `force` bilan. Topilgan xato: spacewm o'z muhitini (`setenv`) Mesa thread'lari ishlab
turganda o'zgartirar edi — bu data race; endi o'zgaruvchilar bolalarga aniq beriladi.
Protokol: docs/ipc.md.

## 7.4-bosqich: layer-shell, HUD, launcher

Layer surface — oyna emas: mijoz monitorning chetiga yopishishni so'raydi (waybar: tepada,
to'liq kenglikda, 30 px) va to'rt qatlamdan biriga tushadi. 3D'da ular sahna ustidan tekis
2D qatlam bo'lib chiziladi (overlay > top > HUD > bottom); background chizilmaydi — fonimiz
olamning o'zi. Exclusive zone ekranda joy band qiladi: HUD va 2D'da kattalashtirilgan
oynalar bar ostidan boshlanadi. Klaviaturani "exclusive" so'ragan qatlam (fuzzel) uni
oladi, yurish to'xtaydi va oddiy 2D kursor paydo bo'ladi; Mod+Escape klaviaturani har doim
qaytarib oladi. Frame callback'lar qatlamlarga ham yuboriladi — aks holda waybar bir marta
chizib qotib qolardi. O'rnatilgan launcher .desktop fayllarni har ochilganda qayta o'qiydi
(field code'lar olib tashlanadi, NoDisplay/Hidden/TryExec hisobga olinadi) va fuzzy
qidiradi: harflar tartib bilan kelishi kerak, so'z boshi va ketma-ket harflar ko'proq ball
oladi ("ffx" → Firefox, "gc" → GNOME Calculator).

## 7.5-bosqich: desktop protokollari

- **xdg-decoration**: doim server-side so'raymiz — sarlavha chizilmaydi, panel ramkasi
  bezak vazifasini bajaradi; client-side'ni talab qilgan ilovaga ruxsat (testda shunday).
- **Ekran tasviri**: ext-image-copy-capture (grim, portal-wlr) va wlr-screencopy
  (wf-recorder; smithay'da yo'q, o'zimiz yozdik). Tayyor kadr (3D + overlay) o'qib olinib,
  mijozning shm buferiga yoziladi. `copy_with_damage` keyingi o'zgarishni kutadi — jim
  ekran bir xil kadrlarni yozmaydi.
- **Qulf (ext-session-lock)**: faqat locker surface chiziladi, hamma input unga; qulf
  kadr chizilgandan keyingina tasdiqlanadi; swaylock o'ldirilsa ekran qizil "locked"
  bo'lib qoladi va faqat yangi locker oladi — aks holda istalgan jarayoningiz
  `pkill swaylock` bilan desktopni ochib qo'yardi. Fokus faqat `set_keyboard_focus` orqali
  o'zgaradi va qulf paytida boshqa maqsadni rad etadi.
- **Idle**: har input swayidle taymerini nolga qaytaradi; mpv video paytida inhibitor ushlaydi.
- **Kichiklari**: viewporter (mpv'ga kerak edi), fractional-scale, pointer-constraints
  (o'yin sichqonchani "qulflasa" panel kursori to'xtaydi), cursor-shape, xdg-activation
  (fokusni o'g'irlamaydi — panel ramkasi 3 s sariq pulsatsiya qiladi), presentation-time
  (DRM'da haqiqiy vblank vaqti), primary selection.
- **Tekshirilmagan**: portal-wlr kadrni oladi, lekin portal orqali screenshot/Firefox
  ekran ulashish headless'da oxirigacha tasdiqlanmadi (ScreenCast PipeWire talab qiladi).

## 7.6-bosqich: unumdorlik

O'lchov: `tests/perf/bench.sh` (headless, 1920x1080, 5 ta har 50 ms yozadigan terminal +
Firefox'da 720p video), `spacectl stats` (kadr CPU vaqti persentillari, sekundiga panel
yangilanishlari, GPU xotirasi bahosi).

| holat | oldin | keyin |
|---|---|---|
| bo'sh desktop, CPU | 0.0% | 0.0% |
| 6 faol panelga qarab: CPU / kadr p95 | 23.3% / 6.3 ms | 22.6% / 7.6 ms (o'lchov shovqini) |
| panellardan teskari qarab: CPU | 25.6% | **0.0%** |
| teskari qarab: panel yangilanishlari/s | 122 | **0** |

Nima qilindi: kamera ko'rmaydigan (yoki Mod+H bilan yashirilgan) panellarning teksturasi
yangilanmaydi — `content_dirty` saqlanib, ko'ringanda yangilanadi — va ilovalari frame
callback'ni sekundiga 1 marta oladi (ishlashda davom etadi, lekin hech kim ko'rmaydigan
kadrlarni chizmaydi); 10 m dan uzoqdagi panellar yarim aniqlikda (xotiraning ¼ qismi);
bir kadrda ko'pi bilan 8 ta panel va 4 ta chunk yangilanadi; kadr bosqichlari uchun
tracing span'lari (`--features profile` → tracy, yoki `SPACEWM_SPAN_TIMES=1`).
Profil ko'rsatdi: ko'rinib turgan faol terminalda vaqtning asosiy qismi — client shm
buferini GPU'ga yuklash (~1 ms/kadr, terminal butun ekranni aylantiradi); sahna 0.5 ms,
mipmap 0.01 ms. Direct scanout — TODO (drm.rs da izoh).

### Oyna o'lchamini o'zgartirish (Mod+Ctrl+= / Mod+Ctrl+-)

`Mod+=` / `Mod+-` panelni dunyoda kattaroq yoki kichikroq **ko'rsatadi**, lekin oynaning
pikseli o'zgarmaydi: terminalda qatorlar va ustunlar soni o'shandayligicha qoladi.
`Mod+Ctrl+=` / `Mod+Ctrl+-` esa oynaning o'zini 10% ga kattalashtiradi yoki kichiklashtiradi:
spacewm ilovaga yangi o'lcham bilan `xdg_toplevel.configure` yuboradi (X11 ilovaga
`ConfigureWindow`). Ilova shu o'lchamdagi buferni chizadi, panel esa markazi atrofida
o'sadi. Aniq o'lcham kerak bo'lsa: `spacectl resize <id> 1400 900`. Ilova o'lchamni biroz
yaxlitlashi mumkin (foot uni katak o'lchamiga moslaydi).

Shu ish jarayonida topilgan xato: ko'rinmaydigan panellar frame callback'ni "sekundiga 1
marta" olishi kerak edi, lekin callback'lar faqat chizilgan kadrdan keyin yuborilardi.
Sahna harakatsiz bo'lsa kadr bo'lmaydi, shuning uchun bunday ilovalar butunlay to'xtab
qolardi (masalan, resize qilingan terminal yangi o'lchamni hech qachon chizmasdi). Endi
sekundiga bir marta ishlaydigan timer bu callback'larni render qilmasdan yuboradi.

### Alt+Enter'dan keyin "qotib qolish" (tuzatildi)

Belgi: Alt+Enter bosilgach, ekran o'zgarmaydi, sichqoncha kamerani burmaydi, yurib
bo'lmaydi. spacewm aslida ishlab turgan edi (Alt+Shift+E ishladi).

Sabab: siz so'ragan oyna darhol Window rejimiga olinadi (tugmalar unga ketadi), lekin
layout.json'dagi saqlangan joyga ham qo'yilardi. Bu joy esa orqangizda bo'lishi mumkin.
Natijada ko'rinmaydigan terminalga yozardingiz va sichqoncha kamerani emas, o'sha
terminaldagi kursorni surardi.

Tuzatish: so'ralgan oyna saqlangan joyga faqat o'sha joy hozir ko'rinib turgan bo'lsa
qo'yiladi. Buning uchun panel markazi kadrda, old tomoni sizga qaragan va 8 m dan yaqin
bo'lishi kerak. Aks holda oyna ro'parangizda ochiladi, saqlangan joy esa keyingi safar
uchun esda qoladi. Ishga tushganda o'zi ochiladigan oynalar (autostart) avvalgidek
saqlangan joylariga qaytadi.

Yo'l-yo'lakay topilgan xato: oyna yopilganda uning joyi esda qolmasdi, chunki
`toplevel_destroyed` chaqirilganda app_id allaqachon tozalangan bo'ladi. Endi panel oxirgi
app_id va sarlavhani o'zida saqlaydi (`Panel::names`).

### Qurilish: hotbar, inventar (E), oddiy klik bilan blok qo'yish

Minecraft'dagidek:
- **Hotbar**: ekran pastida 5 ta slot. Grass Block, Stone, Oak Planks, Bricks, Glass.
  Oak Planks va Bricks yangi bloklar, ularning teksturalari ham kodda chiziladi
  (`render/atlas.rs`). Tanlangan slot oq ramkada turadi. Faqat Walk rejimida ko'rinadi.
- **1-5** tugmalari va **sichqoncha g'ildiragi** slot tanlaydi. Panel ko'tarib
  yurganingizda g'ildirak avvalgidek panelni yaqinlashtiradi yoki uzoqlashtiradi.
- **E**: inventar ochiladi. O'rtada kulrang quti chiqadi, sichqoncha esa kamerani emas,
  oddiy kursorni suradi. Blokni bossangiz, u tanlanadi va quti yopiladi. E yoki Escape
  bilan ham yopiladi. Bu tugmalar faqat Walk rejimida ishlaydi; oyna ichida E oddiy
  harf bo'lib qoladi.
- **O'ng klik** tanlangan blokni qo'yadi. **Chap klik** blokda bo'lsa uni sindiradi,
  panelda bo'lsa avvalgidek oynaga kiradi. Mod+klik ham ishlashda davom etadi.

Ikonkalar CPU'da "inverse mapping" usulida chiziladi: ikonkaning har bir pikseli
uchun kubning qaysi yuzasiga va teksturaning qaysi pikseliga tushishi hisoblanadi.
Yuqori yuza yorug'roq, yon yuzalar qoraroq, shunda kub yuqoridan yoritilgandek ko'rinadi.

### Fullscreen (Mod+F)

Panelga qarab yoki oyna ichida turib Mod+F bossangiz, ilovaga ekran o'lchami va
`fullscreen` holati yuboriladi. X11 ilovalari `_NET_WM_STATE_FULLSCREEN` oladi.
spacewm bu paytda 3D dunyoni chizmaydi: oyna butun ekranga tekis chiziladi, bitta oyna
pikseli bitta ekran pikseliga to'g'ri keladi, kursor ham oddiy 2D kursor bo'ladi.
Chiqish usullari:
- Mod+F yana: oynada qolasiz;
- Mod+Escape: yurish rejimiga qaytasiz;
- ilovaning o'zi so'raganda.

Oyna eski o'lchamiga qaytadi. Ilova o'zi so'raganda ham (Firefox'da F11, YouTube'dagi
fullscreen tugmasi, `mpv --fs`) shu mexanizm ishlaydi, lekin faqat siz ishlatayotgan
oyna uchun. Fon'dagi dastur ekranni egallab ololmaydi. Fullscreen paytida Waybar kabi
panellar yashiriladi.

### Mod+Tab / Mod+` : keyingi oyna

Kamera keyingi oynaga buriladi. Oyna yaqin bo'lsa (reach masofasida, old tomoni
sizga qaragan), siz darhol uning ichiga kirasiz. Uzoq bo'lsa, shunchaki unga qarab
turasiz. Dunyoda "yo'qolib qolgan" oynani topishning eng oson yo'li shu. Nested
rejimda Alt+Tab'ni Hyprland o'zi oladi, shuning uchun Alt+` (Tab ustidagi tugma)
ham bog'langan.

Yo'l-yo'lakay tuzatilgan xato: yangi ochilgan oynaga kirganda kursor oyna o'rtasida
emas, chap yuqori burchakda boshlanardi. Sababi, oynada hali bufer yo'q edi va
o'lchami 0 deb olinardi.

### Alt+K: barcha tugmalar ekranda

Alt+K bosilganda spacewm ishlatadigan barcha tugmalar ikki ustunda chiqadi. Ro'yxat
amaldagi konfiguratsiyadan olinadi: config'da tugmani o'zgartirsangiz, bu yerda ham
o'zgaradi. Siz qo'shgan `[[keybinds.spawn]]` tugmalari ham ko'rsatiladi. "Mod" esa
sizda haqiqatda qaysi tugma bo'lsa, o'sha nom bilan yoziladi: nested rejimda Alt, haqiqiy
sessiyada Super. Oyna ochiq paytda bosilgan tugmalar ilovaga yetib bormaydi, kamera
ham qimirlamaydi. Esc, Alt+K yoki sichqoncha bosilsa yopiladi. Bu Window rejimida ham,
fullscreen'da ham ishlaydi.

### Dastur tashqarida ochiq bo'lsa ham spacewm ichida ochiladi (src/isolate.rs, src/desktop/elsewhere.rs)

Muammo: Steam, Telegram, Firefox, Nautilus kabi ko'p dasturlar "bitta nusxali". Ularni
ikkinchi marta ishga tushirsangiz yangi oyna ochilmaydi: yangi nusxa ishlab turgan
nusxaga "oyna och" deb xabar yuboradi va yopiladi. O'sha nusxa Hyprland'da ishlayotgan
bo'lsa, oyna spacewm'da emas, Hyprland'da ochiladi. Ular bir-birini turli yo'llar bilan
topadi: D-Bus nomi, profil papkasidagi socket yoki lock, Steam'dagi `~/.steam/steam.pipe`.

Avval spacewm buni har bir dastur uchun alohida hal qilardi: Firefox va Telegram'ga
alohida profil, Nautilus'ga shaxsiy D-Bus berilardi. Steam esa hech bir ro'yxatda yo'q
edi va baribir Hyprland'da ochilardi. Endi **dasturdan qat'i nazar bitta umumiy
yechim** ishlaydi, ro'yxat ham, alohida kod ham yo'q:

1. **Kuzatish** (`desktop/launching.rs`): siz ochgan dastur oynasi chiqquncha spacewm
   uning jarayonlar daraxtini birinchi 5 soniya davomida har 50 ms'da kuzatib turadi.
   Bunda daraxtdagi har bir dastur nomi `/proc/<pid>/exe` dan olinadi, shuning uchun
   skript ichidan chaqirilgan haqiqiy dastur ham ko'rinadi. sh, bash va python kabi
   yordamchilar hisobga olinmaydi.
2. **Aniqlash** (`isolate.rs`): dastur oyna ochmasdan 0 kodi bilan chiqib ketsa, spacewm
   o'sha dasturlardan birining tashqaridagi nusxasini qidiradi. Nusxa quyidagi
   shartlarga mos kelishi kerak:
   - sizga tegishli;
   - spacewm'ning bolasi emas;
   - siz dasturni ochishdan **oldin** ishga tushgan (`/proc/<pid>/stat` dagi
     starttime orqali).

   Shunday nusxa topilsa, demak so'rov unga uzatilgan. Terminal kabi oddiy dasturlar
   esa oyna ochadi, shuning uchun ularga hech qachon tegilmaydi.
3. **Ko'chirish** (`desktop/elsewhere.rs`): spacewm'da savol chiqadi: "Steam spacewm'dan
   tashqarida ochiq. U yerda yopib, shu yerda ochaymi?"
   - **Enter** yoki tugma bosilsa: tashqaridagi nusxaga SIGTERM yuboriladi. Bu
     oynani yopish bilan bir xil: dastur ishini saqlab, o'zi yopiladi. spacewm uning
     yopilishini kutadi (ko'pi bilan 30 s) va dasturni spacewm ichida qayta ochadi.
     Endi so'rovni uzatadigan nusxa qolmagani uchun dastur shu yerda ochiladi, sizning
     odatdagi profilingiz bilan: login, tablar va sozlamalar joyida qoladi. Majburan
     o'ldirish (SIGKILL) hech qachon ishlatilmaydi. Dastur u yerda "saqlaysizmi?" deb
     so'rab qolsa, spacewm shuni aytadi.
   - **Esc**: hech narsa qilinmaydi.

   Sozlama: `[desktop] running_elsewhere`:
   - `"ask"` (standart): har safar so'raydi;
   - `"move"`: so'ramasdan ko'chiradi;
   - `"leave"`: dasturni o'sha yerda qoldirib, faqat xabar beradi.

Eski alohida profillar (`~/.local/share/spacewm/apps/`) endi ishlatilmaydi, ularni
o'chirib tashlasangiz ham bo'ladi.

Test (`tests/e2e/stage8_isolation.sh`) alohida `dbus-run-session` va vaqtinchalik HOME
ichida ishlaydi. U o'zi yaratgan `spacefake` nomli soxta "bitta nusxali" dasturni ikkinchi
spacewm'da ochadi, keyin uni ko'chiradi. Haqiqiy dasturlaringizga tegmaydi.

### Barqarorlik: stress testi (tests/e2e/stage8_stress.sh)

Har bir siklda quyidagilar bajariladi:
- foot, weston-simple-shm va xterm (X11) ochiladi;
- fullscreen ikki marta yoqib-o'chiriladi;
- har 3-siklda xterm `kill -9` bilan o'ldiriladi;
- inventar, yordam oynasi va keyingi oynaga o'tish ishlatiladi, bloklar qo'yiladi;
- hamma oynalar yopiladi.

100 sikl (300 ta oyna) natijasi: ochiq fayllar 33 → 33, xotira 83.4 → 85.2 MB (oyna
boshiga taxminan 6 KB, bu allocator keshlari, oqish emas), panic va xato yo'q.

### Kunlik ishlatish uchun (src/desktop/)

spacewm'ni yagona, asosiy desktop qilib ishlatish uchun:

- **Media tugmalar**: ovoz +/-/mute, mikrofon, ekran yorug'ligi, play/pause/next/prev.
  Har bosishda pastda kichik OSD chiqadi ("Volume 55%" va chiziq) va 1.6 soniyadan
  keyin yo'qoladi. Ular har qanday rejimda ishlaydi, qulflangan ekranda ham: musiqani
  ochmasdan pasaytirish mumkin. Boshqa tugmalar qulf ostida ishlamaydi.
- **Screenshot**: Print butun ekranni, Shift+Print sichqoncha bilan belgilangan hududni
  (slurp orqali) oladi. Fayl `~/Pictures/Screenshots/spacewm-SANA_VAQT.png` ga saqlanadi
  va clipboard'ga ham ko'chiriladi, shuning uchun darhol Telegram'ga Ctrl+V qilish
  mumkin.
- **Qulflash**: Mod+L. Haqiqiy sessiyada 10 daqiqa harakatsizlikdan keyin avtomatik
  qulflanadi. Video ko'rayotganda bunday bo'lmaydi, chunki pleyerlar "idle inhibitor"
  ushlab turadi. Nested rejimda avto-qulf o'chiq: u yerda qulflashni Hyprland qiladi.
- **HUD**: "16:49  Sat 24 Oct   Walk   Bat 87%+   EN". "+" quvvat olayotganini bildiradi.
  Til tegi faqat bir nechta klaviatura tili sozlangan bo'lsa chiqadi. Batareya 10% ga
  tushsa va zaryadka ulanmagan bo'lsa, bir marta qizil ogohlantirish chiqadi.
- **Sessiyani tiklash**: spacewm yopilganda qaysi dasturlarning oynasi ochiq bo'lgani
  layout fayliga yoziladi. Keyingi ishga tushishda ular qayta ochiladi, oynalar esa
  o'z joylariga qaytadi. Ikkita foot ikkita bo'lib qaytadi, Firefox esa bitta jarayon
  bo'lgani uchun bir marta ochiladi. Faqat dastur fayli saqlanadi, argumentlar
  saqlanmaydi: `foot -e buyruq` qayta ochilganda o'sha buyruq yana bajarilmaydi.
- **Haqiqiy sessiyada**: bildirishnoma daemon'i bo'lmasa (`org.freedesktop.Notifications`
  egasi yo'q bo'lsa), mako ishga tushadi. Session skripti Electron dasturlarini
  Wayland'da ishlashga yo'naltiradi.

Barcha buyruqlar `[desktop]` bo'limida, masalan wpctl o'rniga pactl qo'yish mumkin.
Buyruqlar alohida thread'da, 4 soniyalik timeout bilan bajariladi: osilib qolgan
vosita desktop'ni qotira olmaydi. Testlar (`stage9_desktop.sh`) soxta skriptlardan
foydalanadi va haqiqiy ovoz yoki yorug'likni o'zgartirmaydi.

## Qulaylik paketi

Maqsad: spacewm'ni tugma yodlamasdan, sichqoncha bilan va adashmasdan ishlatish.

- **"Starting Firefox …"**: siz ochgan dastur (launcher, terminal tugmasi, dock, o'z
  tugmalaringiz) oynasi chiqquncha tepada ko'k qatorda yoziladi. Qator 0.4 soniyadan
  keyin chiqadi, shuning uchun foot kabi tez dasturlarda miltillamaydi. Dastur xato kodi
  bilan yopilsa ("could not start (exit code 3)") yoki umuman topilmasa, qizil xabar
  chiqadi. Yolg'on ogohlantirish bo'lmaydi: oynasiz normal yopilgan dastur jimgina
  ro'yxatdan chiqadi.
- **Kirganda avtomatik tiniqlik**: uzoqdagi oynaga kirsangiz, kamera 0.35 soniyada uning
  ro'parasiga silliq keladi. Masofa matn piksel-aniq ko'rinadigan qilib tanlanadi.
  Chiqqanda kamera aynan siz turgan joyga qaytadi. O'yinchining o'zi joyidan
  qimirlamaydi. Agar siz oynaga o'zingiz yaqin kelgan bo'lsangiz, kamera sizni orqaga
  tortmaydi. Sozlama: `panels.focus_zoom`.
- **Oyna menyusi**: oynaga qarab o'ng klik qilsangiz menyu chiqadi (oyna ichida:
  Alt+o'ng klik). Undagi bandlar: ishlatish, fullscreen, ko'chirish, kattaroq/kichikroq
  ko'rsatish, qator ko'proq/kamroq, burish, "menga qara", yashirish, yopish. Har bandning
  yonida tugmasi yozilgan, shunda tugmalar o'z-o'zidan o'rganiladi.
- **Sichqoncha bilan surish va cho'zish**: oynaning tepa chetini ushlab torting, oyna
  ko'chadi. Devorga qaratsangiz, devorga yopishadi. Pastki o'ng burchakni tortsangiz,
  oyna o'lchami o'zgaradi. Shu joylarni shunchaki bossangiz, avvalgidek oynaga kirasiz.
  Kursor shu joyga kelganda crosshair ostida "drag: move" yoki "drag: resize" chiqadi.
- **Dock**: pastda sevimli dasturlar (`[dock] apps`) va hozir ochiq dasturlar turadi,
  ochiqlari ostida nuqta bo'ladi. Ikonkalar tizim ikonka mavzusidan olinadi. Alt'ni
  bosib turing, oddiy kursor chiqadi (o'yinlardagidek). Chap klik: ochiq dasturning oynasi
  oldingizga keladi (takror bossangiz keyingi oynasi), yopiq bo'lsa ishga tushadi.
  O'ng klik har doim yangi nusxa ochadi. Blok bilan ishlaganda pastda 4 soniya
  hotbar chiqadi, keyin yana dock qaytadi.
- **O'tirgan rejim (Alt+S)**: barcha oynalar atrofingizga yoy shaklida, o'qish
  masofasida, sizga qaragan holda silliq teriladi. Chap-o'ng tartibi saqlanadi.
  Yurish o'chadi, faqat boshingizni burasiz. Yangi oyna yoyga qo'shiladi, yopilgan oyna
  o'rnida bo'sh joy qolmaydi. Alt+S yana bosilsa, oynalar eski joylariga qaytadi.
- **2D/3D (Alt+D)** (keyinchalik olib tashlandi, pastga qarang): ish paytida oddiy 2D desktop'ga o'tish va qaytish mumkin. Har bir
  oyna doim 2D joyiga ham ega, shuning uchun hech narsa yopilmaydi va siljimaydi. 2D'da
  oynalarning sarlavha qatori yo'q, shuning uchun Alt+tortish oynani suradi, Alt+o'ng
  tugma bilan tortish esa o'lchamini o'zgartiradi.
- **Sozlamalar (Alt+,)**: eng ko'p o'zgartiriladigan 14 ta sozlama. Bosish (o'ng tugma:
  orqaga) yoki strelkalar bilan o'zgartiriladi. Qiymat config fayliga `toml_edit` orqali
  yoziladi: faqat o'sha qator o'zgaradi, izohlar va tartib saqlanadi. Yozishdan oldin
  tekshiriladi, atomar yoziladi va darhol qo'llanadi.
- `spacectl camera`: kamera qayerda turgani va qayerga qarayotgani.

### Foydalanuvchi fikridan keyin

- **Siz ochgan oyna doim ro'parangizda ochiladi.** Avval yopilgan terminalning joyi
  esda qolardi va yangisi o'sha yerda, ba'zan uzoqda ochilardi. Endi saqlangan joylar
  faqat spacewm qayta ishga tushganda avtomatik qayta ochiladigan dasturlar uchun
  ishlatiladi.
- **Qurilish standart holatda o'chiq** (`general.building = false`). E inventar, hotbar,
  1–5 tugmalari, blokni buzish va qo'yish ishlamaydi, ya'ni klik dunyoni tasodifan
  o'zgartira olmaydi. Pastda faqat dock turadi. Xohlasangiz, sozlamalardan (Alt+,)
  "Building blocks" ni yoqing.

### Qo'shimcha qulayliklar

- **Oynalar ro'yxati (Mod+Tab / Mod+`)**: Mod'ni bosib turib ` (yoki Tab) bosing.
  Barcha oynalar ikonka, nom va sarlavha bilan ro'yxat bo'lib chiqadi.
  Oxirgi ishlatilgan oyna tepada turadi, kursor esa oldingi oynada bo'ladi. ` ni
  takror bossangiz pastga tushasiz, Mod'ni qo'yib yuborsangiz tanlangan oynaga
  o'tasiz va oyna oldingizga keladi. Strelkalar, Enter, sichqoncha va Esc (bekor qilish) ham ishlaydi.
  Avvalgi "keyingi oynaga burilish" o'rnini shu egalladi.
- **Xush kelibsiz**: spacewm haqiqiy foydalanishda birinchi marta ochilganda tugmalar
  ro'yxati (Alt+K oynasi) "Welcome to spacewm!" sarlavhasi bilan o'zi chiqadi. Bu bir
  marta bo'ladi: `~/.local/state/spacewm/welcomed` belgisi yaratiladi. Testlarda chiqmaydi.

### Alt+D va 2D olam olib tashlandi

Foydalanuvchiga yoqmagani uchun Alt+D (2D desktop: workspace'lar, tiling, tepa panel,
2D'dagi Mod+tortish) to'liq olib tashlandi. spacewm faqat 3D olamda ishlaydi.
`spacewm --render 2d` buyrug'i faqat dasturchilar va testlar uchun oddiy rejim sifatida
qoldi, unga tugma bilan o'tib bo'lmaydi. Alt+1…5 yana faqat qurilish yoqilganda blok
tanlaydi.

### Oynani egish (qiyalik)

Haqiqiy monitorni kronshteynda o'zingizga qaratib qo'ygandek, oynani oldinga yoki
orqaga egish mumkin.

- **Avtomatik**: oynani ko'tarib yurganingizda (tepa chetidan tortish yoki Alt+G)
  u o'zi sizga qarab egiladi: baland qo'ysangiz pastga, past qo'ysangiz yuqoriga
  qaraydi. Ko'z balandligiga yaqin (±8°) bo'lsa tik qoladi, shunda ozgina pastga
  qaraganingiz uchun hamma oyna qiyshayib qolmaydi. Devorga yoki yerga qo'yilganda
  faqat qo'lda berilgan qiyalik qoladi. O'chirish uchun `[panels] auto_tilt = false`
  yoki Alt+, ichidagi "Carried windows face me" bandi.
- **Qo'lda**: Alt+PageDown oynani 15° pastga, Alt+PageUp 15° yuqoriga egadi. Eng
  ko'pi 75°, undan ortiq egilganda oyna yonidan ko'rinib qoladi. O'ng klik menyusida
  ham "Tilt: look down" va "Tilt: look up" bandlari bor. Alt+Shift+R ("Face me,
  upright") oynani sizga qaratib, yana tik qiladi.
- **Hisob**: oynaning burilishi Ry(yaw)·Rx(tilt). Rx(t) +Z ni (0, −sin t, cos t) ga
  o'tkazadi, shuning uchun musbat qiyalikda oyna pastga qaraydi. Avtomatik qiyalik
  `atan2(−dy, gorizontal masofa)`, bu yerda d = ko'z − oyna. Klik va sichqoncha egilgan
  tekislik bo'yicha hisoblanadi, shuning uchun egilgan oynada ham aniq ishlaydi.
  Qiyalik layout'da saqlanadi, `spacectl --json list` da `tilt_deg` sifatida ko'rinadi.

### Oyna o'lchamini sichqoncha bilan o'zgartirish (qaytarildi)

Oynaning pastki o'ng yoki pastki chap burchagiga qarab, sichqonchani bosib turing.
Shu paytda ko'rinish joyida qotib turadi, sichqoncha harakati esa faqat oyna
o'lchamini o'zgartiradi. Burchak sichqoncha bilan birga yuradi, qarama-qarshi tepa
burchak joyida turadi: o'ng burchakda tepa-chap, chap burchakda tepa-o'ng. Shuning
uchun qancha katta bo'layotgani ko'rinib turadi.

Sichqonchaning piksel harakati oynaga qadar bo'lgan masofaga qarab oyna piksellariga
aylantiriladi: `ppu / (ekran_balandligi / (2·masofa·tan(fov/2)))`. Shu tufayli burchak
kursor bilan bir xil tezlikda yuradi. O'lcham 240×140 dan 8192×8192 gacha (GPU teksturasi bemalol sig'adigan chegara).
Undan kattaroq oynaga orqaga chekinib qarash mumkin.

Avvalgi "burchak nishon chizig'iga ergashadi" usuli foydalanuvchiga noqulay bo'lgani
uchun olib tashlandi.

### Dust xaritasi: cho'l shaharchasi (src/world/map.rs, src/render/map.rs)

Endi standart olam ikki bombalash joyi bo'lgan klassik xaritalar uslubidagi cho'l
shaharchasi. Unda quyidagi joylar bor: T spawn (janubda), Long A va uning darvozalari,
balandroqda joylashgan A site, Mid va mid darvozalari, Catwalk, CT spawn, B darvozalari,
B site, yuqori va pastki tunnellar. Voksel (Minecraft) olam ham saqlanib qolgan:
Esc → World → Blocks.

**Assetlar (tayyor, internetdan, CC0):** 8 ta material teksturasi Poly Haven'dan
olingan. Ular qumtosh, suvoq, hoshiya toshi, qum, yo'lak toshi, quti yog'ochi, darvoza
yog'ochi va bo'yalgan metall. Har biriga normal map ham bor. Hammasi 512×512 JPEG,
jami 1.3 MB, `resources/maps/dust/LICENSE.md` da ro'yxati bor. Hech qaysi o'yindan
geometriya, tekstura yoki nom ko'chirilmagan. Asl Dust2 Valve'ga tegishli bo'lgani uchun
uning faqat uslubi olingan.

**Xarita fayli** (`resources/maps/dust/map.txt`): 2 metrlik kataklardan iborat panjara
(48×52 katak). U qattiq devordan buyruqlar bilan "o'yib" chiqiladi:
- `fill`: bir xil turdagi to'rtburchak, masalan `.` qum, `,` yo'lak toshi, `A` arka,
  `T` tunnel, `c`/`C` qutilar, `d`/`D` darvoza tavaqasi, `w` past devor, `m` metall
  do'kon qutisi.
- `stairs`: balandligi bosqichma-bosqich o'sadigan zina, har bosqichi 0.35 m.
- `spawn`: boshlanish nuqtasi.

**Greedy meshing:** bir xil qo'shni kataklar eng katta to'rtburchaklarga birlashtiriladi.
Natijada butun xarita bor-yo'g'i **~140 ta quti va ~1500 ta uchburchakdan** iborat.
Shu qutilar ham ko'rinadigan geometriya, ham to'qnashuv uchun ishlatiladi, shuning uchun
ko'rinish va fizika hech qachon bir-biridan farq qilmaydi. Teksturalar dunyo
koordinatalari bo'yicha qo'yiladi, shuning uchun devor teksturasi bir qutidan
ikkinchisiga uzilmasdan davom etadi. Devorlarning balandligi va rangi biroz farq qiladi.
Devor tepasida tosh karniz bor.

**Render (kompyuterga deyarli yuk yo'q):**
- Butun xarita **bitta draw call** bilan chiziladi.
- Teksturalar texture array'da saqlanadi, mipmap va 8× anizotropik filtr bilan. Shu
  tufayli pol past burchakdan qaralganda ham tiniq ko'rinadi.
- Yorug'lik: quyosh + osmon va qumdan qaytgan yorug'lik (hemisphere ambient) + normal
  map + devor tagidagi qorong'ilik (AO). Yorqin joylar oqarib ketmasligi uchun yumshoq
  tonemap ishlatiladi.
- **Soyalar:** quyoshdan qaralgan chuqurlik rasmi (2048² shadow map). U faqat xarita
  yuklanganda va kun vaqti o'zgarganda bir marta chiziladi, har kadrda emas. Shuning uchun
  soyalar deyarli bepul.
- **Osmon:** gradient (ufqdan zenitgacha) va quyosh atrofidagi yorug'lik. U bitta to'liq
  ekranli uchburchak bilan chiziladi.
- Hech narsa qimirlamasa, kadr umuman chizilmaydi (testda tik turganda 0 fps).

**Fizika:** o'yinchi endi `Solids` interfeysi orqali to'qnashadi: voksel kataklar ham,
xarita qutilari ham shu bitta interfeys orqali ishlaydi. Xaritada 0.45 m gacha
balandlikdagi zinalarga sakramasdan chiqiladi. Zinadan tushayotganda o'yinchi yerdan
uzilmaydi va kamera har zinada sakramaydi, silliq suzib boradi (2.5 m/s).

**Esc menyusi:** Walk rejimida Esc bosilsa, o'yindagi kabi Settings ochiladi. Yangi
bandlar:
- **World:** Desert town / Blocks. Olam spacewm'ni qayta ishga tushirmasdan almashadi.
- **Time of day:** Day / Afternoon / Evening. Quyosh, osmon va soyalar o'zgaradi.
- **Sun shadows:** soyalarni yoqish yoki o'chirish.

Bular config'ning `[world]` bo'limiga yoziladi (`map`, `time_of_day`, `shadows`).

**Olam almashganda oynalar:** har bir oynaning joyi qaysi olamda saqlangani bilan birga
yoziladi (layout.json'da `world`). Olam almashganda ochiq oynalar o'sha olamdagi eski
joyiga qaytadi. Agar u olamda hali bo'lmagan bo'lsa, oldingizga yoy shaklida teriladi.
Voksel olamdagi joylar o'zgarishsiz saqlanib qoladi.

Test: `tests/e2e/stage13_dust.sh` quyidagilarni tekshiradi:
- yuklanish va uchburchaklar soni;
- tik turganda 0 fps;
- devor to'xtatishi;
- A site'ga zinadan chiqish;
- oynani devorga osish;
- Esc menyusi;
- kun vaqti almashganda soyaning qayta chizilishi;
- olamni jonli almashtirish.

Boshqa testlar voksel olamga moslab yozilgani uchun `--map voxel` bilan ishga tushadi.

### Yangi oyna qo'lingizda ochiladi

Tugma, launcher yoki dock orqali ochilgan yangi oyna Alt+G bosilgandek darhol qo'lingizga
beriladi. U o'qish masofasidan ikki baravar uzoqroqda, 1.2–3 m oralig'ida turadi.
Qayerga qo'ymoqchi bo'lsangiz, o'sha joyga qarab chap klik qiling yoki Alt+G bosing.
G'ildirak oynani yaqinlashtiradi yoki uzoqlashtiradi. Devorga qaratib qo'ysangiz, oyna
devorga osiladi.

Sozlash:
- `[panels] carry_new_windows = false`, yoki Settings'dagi "New windows in my hand" bandi.
  O'chirilsa, oyna avvalgidek o'qish masofasida, yozishga tayyor holda ochiladi.
- Skript va IPC orqali ochilgan oynalar ham shunday ochiladi. Testlar buni
  `--no-carry-new-windows` yashirin bayrog'i bilan o'chiradi.

### Tana: F5 (ko'rinish) va F6 (animatsiyalar) (src/world/character.rs, body.rs)

**Personaj:** KayKit Adventurers to'plamidagi Knight, CC0. U `resources/characters/`
papkasida, o'z LICENSE fayli bilan. Bitta `.glb` faylda mesh, 41 suyakli skelet,
tekstura va 76 ta animatsiya bor. `gltf` crate faqat parser sifatida ishlatiladi.
Qurollar olib tashlangan, dubulg'a va plash qolgan. Ular bosh va ko'krak suyagiga 100%
bog'langan "qattiq" qismlar sifatida chiziladi, shuning uchun butun tana bitta mesh,
~4700 uchburchak va bitta draw call.

**Skeletli animatsiya qanday ishlaydi:**
1. Har bir vertex 4 tagacha suyakka og'irlik (weight) bilan bog'langan.
2. Animatsiya har bir suyakning siljishi, aylanishi va masshtabi uchun keyframe'lardan
   iborat. Hozirgi vaqt t uchun suyak holati qo'shni keyframe'lar orasida interpolyatsiya
   qilinadi: siljish va masshtab uchun `lerp`, aylanish uchun `slerp`.
3. Suyaklar daraxt bo'ylab ko'paytiriladi: `ota * bola`.
4. Har bir suyak uchun `global * inverse_bind` matritsasi vertex shader'ga beriladi.
   `inverse_bind` suyak mesh'ga ulangan paytdagi holatni bekor qiladi.
5. GPU vertex'ni og'irliklari bo'yicha 4 ta matritsa aralashmasi bilan suradi.

Animatsiyalar orasidagi o'tish 0.2 s davom etadi (crossfade).

**F5** (Walk rejimida) ko'rinishni almashtiradi: ko'z → orqadan → oldidan → ko'z.
- Faqat surat olinadigan kamera suriladi. Nishon olish, oynani bosish va ko'tarish
  avvalgidek ko'zdan ishlaydi (o'yinlardagi kabi).
- Kamera ko'zdan orqaga qarab nur otadi va devordan 0.25 m oldin to'xtaydi, shuning uchun
  devor ichiga kirib ketmaydi.
- Kamera ko'zdan 0.35 m yuqorida turadi va biroz pastga qaraydi, shunda butun gavda
  ko'rinadi.

Animatsiyalar o'zi tanlanadi: turish, yurish, yugurish, orqaga yurish va havoda.

**F6** 9 ta animatsiyali menyuni ochadi: Cheer, Sit, Lie down, Punch, Kick, Magic, Roll,
Pick up va Fall down.
- Tanlash: 1–9 raqamlari, sichqoncha yoki strelkalar + Enter. Esc yoki F6 menyuni yopadi.
- Sit va Lie down avval o'tirish yoki yotish harakatini qiladi, keyin shu holatda
  tsiklda qoladi. Fall down oxirgi holatda qoladi.
- Yurishni boshlasangiz, animatsiya to'xtaydi.
- Animatsiyani ko'z rejimida tanlasangiz, ko'rinish o'zi oldinga o'tadi, aks holda uni
  ko'rmagan bo'lardingiz.

**Yuk:**
- Ko'z rejimida tana chizilmaydi, turganda 0 fps.
- Uchinchi shaxsda turganingizda Idle animatsiyasi sekundiga 15 kadr bilan ishlaydi,
  uni taymer uyg'otadi.
- Yurganda kadrlar baribir har safar chiziladi.

Test: `tests/e2e/stage14_body.sh`.
