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
