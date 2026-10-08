# Panduan Pengguna

Untuk orang yang memakai WebApp Noctis Team: dari mendaftarkan provider model sampai menerima hasil kerja di cabang Git.
Pemasangan dan konfigurasi server ada di [operations.md](operations.md).

## Cara kerja singkat

1. Anda memberi **tujuan (objective)** dan kriteria penerimaan untuk sebuah repository Git.
2. **Lead** (model) menyusun **plan** berisi beberapa task beserta urutan ketergantungannya.
3. **Anda menyetujui atau menolak plan.** Tidak ada pekerjaan yang berjalan sebelum persetujuan.
4. **Worker** (2–4 paralel) mengerjakan task di worktree terpisah, **reviewer** memeriksa, perintah verifikasi dijalankan di
   container tanpa jaringan, lalu **integrator** menggabungkan hasil ke cabang `noctis-integration-<run>`.
5. Anda meninjau cabang itu dan meng-merge-nya sendiri. Noctis **tidak pernah** push dan tidak mengubah cabang utama.

## 1. Mendaftarkan provider dan model (sekali)

Buka **Providers**.

1. **Create provider**: isi *Provider ID* (nama bebas), *Base URL* provider OpenAI-compatible (mis. `https://api.openai.com/v1`),
   *API key env* (**nama** variable environment di server yang berisi API key; bukan key-nya), dan batas waktu request.
   Key tidak pernah diketik di UI dan tidak pernah ditampilkan. Bila provider ada di localhost/LAN, operator harus
   mengizinkan host-nya agar probe bisa berjalan (`NOCTIS_PROVIDER_HOST_ALLOWLIST`, lihat operations.md).
2. Pilih provider itu lalu **Tambah model**: *Model ID*, *Remote model name* (nama di provider), *Class* (pakai `coding`),
   *Context window*, *Max output tokens*, dan *Claimed capabilities*.
   - **Model ID harus sama persis dengan `provider.model` di konfigurasi server** (bawaan `gpt-5-mini`). Bila beda, task
     tidak akan pernah dijalankan dan tidak ada pesan error. Tanyakan operator nilainya.
   - Worker memerlukan **tool calling**. Jalankan probe *tools* pada model; hanya model yang terverifikasi `supported`
     yang layak dipakai. Probe mengirim request kecil ke provider dan memakai sedikit kuota.

## 2. Mendaftarkan project

Buka **Projects** → **Register repository**: isi nama dan path repository Git **di server** (direktori Git yang sudah ada;
pada Compose berarti path di dalam container, lihat deployment.md). Noctis lalu menampilkan **Repository discovery**:
perintah test yang terdeteksi (mis. `npm test`) yang akan dipakai sebagai verifikasi.

Pastikan repository bersih dan commit-nya sudah ada; worker bekerja dari commit saat ini. Jangan menaruh secret di repository.

## 3. Memulai run

Di halaman project, bagian **Start a run**:

| Kolom | Isi |
|---|---|
| Objective | tujuan dalam satu-dua kalimat; makin spesifik makin baik. Sebutkan batas path bila perlu |
| Acceptance criteria | satu kriteria per baris yang bisa diperiksa (mis. "test pencarian lulus") |
| Token budget | batas total token run. Reservasi plan dihitung dari limit tiap task; plan yang butuh lebih dari sisa budget tidak bisa disetujui |

Klik **Create run**; Anda masuk ke halaman run dengan status **Planning**.

## 4. Plan: tinjau lalu setujui atau tolak

Klik **Ask Lead to plan**. Lead menjawab dengan plan (beberapa detik sampai menit). Status menjadi **Awaiting approval**.

Periksa:
- **Urutan (Step 1, 2, 3…)**: task satu step bisa berjalan paralel; step berikutnya menunggu dependency.
- **Allowed paths** tiap task: worker hanya boleh mengubah file di path itu. Path yang saling tumpang-tindih tidak dijalankan
  bersamaan (task kedua menunggu).
- **Risk flags** dan **Tokens reserved on approval** (peringatan "Approval will be refused" bila melebihi sisa budget).

Lalu:
- **Approve plan**: isi *Actor ID* (nama Anda; dicatat di audit, tidak diverifikasi), centang konfirmasi, tekan **Approve plan**.
  Task dibuat dan scheduler mulai bekerja.
- **Reject plan**: wajib mengisi alasan. Tidak ada task dibuat; Anda dapat meminta **Lead untuk plan baru** (versi 2) dan
  seterusnya.

Plan yang tidak valid (siklus ketergantungan, melebihi budget) ditolak otomatis dengan pesan yang terbaca dan tidak disimpan.

## 5. Memantau

Halaman run menampilkan papan task per status (Queued, Blocked, Running, Review, Done, …), alasan menunggu
("Waiting for … "), panel budget (dipakai / dipesan / sisa), dan worker yang sedang berjalan. Tampilan diperbarui otomatis.

- **Pause** menghentikan pengambilan task baru; **Resume** melanjutkan. **Cancel run…** (dengan konfirmasi) membatalkan run.
- Halaman **task** menampilkan riwayat event, artifact (diff, hasil verifikasi), pemakaian token, dan error terakhir.
- Halaman **Operations** menampilkan kesehatan sistem; **Settings** menampilkan konfigurasi yang berlaku.

## 6. Bila perlu keputusan Anda (Approvals)

Buka **Approvals**, isi *Your name or ID*. Ada dua antrean:

- **Plans waiting for approval**: plan dari semua run.
- **Tasks that need a person**: task berstatus **Conflict** (patch bertabrakan saat integrasi) atau **Needs a human**
  (percobaan habis, atau worker sengaja meminta keputusan Anda karena acceptance tidak dapat dipenuhi/bertentangan; baca
  penjelasan di kartunya). Setiap kartu menampilkan aktivitas terakhir dan
  diff (**Show diff**). Pilihan Anda:
  - **Retry task**: percobaan baru dari salinan bersih. Isi alasan "kenapa aman mengulang"; efek samping percobaan
    sebelumnya tidak diketahui, jadi ulangi hanya setelah memeriksanya. Tombol ini nonaktif sampai integrator menyerahkan
    task ke manusia.
  - **Cancel task**: menghentikan task selamanya; task yang bergantung padanya tetap terblokir. Alasan wajib.

Setiap keputusan membutuhkan konfirmasi dan tercatat atas nama Anda.

## 7. Menerima hasil

Saat semua task `DONE`, hasilnya ada di repository project:

```bash
git -C /path/ke/repository branch --list 'noctis-integration-*'
git -C /path/ke/repository log --oneline main..noctis-integration-<run-id>
git -C /path/ke/repository diff main...noctis-integration-<run-id>
```
Tinjau seperti pull request manusia (agent bisa menulis kode yang keliru atau berbahaya meski test lulus), jalankan test
sendiri, lalu `git merge` bila puas. Cabang tidak dihapus oleh retensi; hanya direktori worktree-nya yang dibersihkan.

## Yang perlu Anda ketahui

- **Tidak ada login.** Siapa pun yang bisa membuka UI bisa menyetujui plan. Jangan ekspos port ke jaringan.
- **Biaya**: setiap run memakai token provider (Lead + worker + reviewer, dan percobaan ulang). Pantau panel budget; isi
  *Token budget* sesuai kemampuan Anda menanggung biaya. Angka perbandingan hemat token vs satu agent ada di
  [benchmark.md](benchmark.md) (status pengukuran tertulis di sana).
- **Verifikasi memerlukan akses Docker** pada server. Tanpa itu task tidak bisa lolos verifikasi (operations.md).
- Image verifikasi satu untuk seluruh server (`NOCTIS_RUNNER_IMAGE`); proyek yang butuh toolchain lain perlu image yang sesuai.
- Run tidak melanjutkan panggilan model yang terputus di tengah; setelah server restart attempt diulang dari awal.
