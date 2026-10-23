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
(`-listenfd`) ishga tushiradi, kutib turgan ulanish yo'qolmaydi. 30 s davomida birorta X
oynasi qolmasa, server to'xtatiladi (~30 MB qaytadi); crash ham xuddi shu yo'l.

**Nega override-redirect oynalar alohida.** X11'da menyu, tooltip, combo ro'yxati WM'dan
ruxsat so'ramaydi: o'zi *root* (global) koordinatada paydo bo'ladi va sichqonchani ushlaydi.
Ilova bu joyni "mening oynam qayerda" degan bilimidan hisoblaydi. 3D'da "ekrandagi joy"
yo'q, shuning uchun har X oynasiga 2D'da qat'iy joy beramiz va aynan shunga configure
qilamiz: menyu joyi − oyna joyi = menyuning panel ichidagi joyi. Ular alohida panel emas,
ota-panel teksturasiga chiziladi va sichqoncha ular ustida bo'lsa input ularga boradi.

**Topilgan xatolar:** (1) signalfd faqat signal *hamma* thread'da bloklansa ishlaydi —
mesher thread'i bloklamagani uchun SIGTERM spacewm'ni tozalamasdan o'ldirar edi; endi
main() boshida bloklanadi. (2) smithay X clipboard egasini o'zgartirgach flush qilmaydi.
