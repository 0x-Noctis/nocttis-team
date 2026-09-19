# Aturan Agent Noctis Team

Instruksi ini berlaku untuk seluruh repository.

## Sebelum Bekerja

1. Baca task lengkap di `docs/task-list.md` dan rancangan terkait di `docs/RANCANGAN.md`.
2. Pastikan seluruh dependency task berstatus selesai.
3. Gunakan branch dan Git worktree terisolasi.
4. Pastikan tidak ada task aktif lain yang memiliki file sama.

## Batas Perubahan

- Ubah hanya `Allowed Paths` milik task.
- Jangan mengubah status tracker; Integrator mengelola `docs/task-list.md`.
- Jangan mengubah kontrak shared, dependency, lockfile, atau migration lama kecuali task mengizinkan.
- Migration bersifat append-only.
- Jangan melakukan push atau merge ke branch utama.
- Pilih implementasi minimum yang memenuhi acceptance criteria.

## Keamanan

- Jangan simpan API key, password, token, credential, atau secret dalam source, database, fixture, artifact, commit, maupun log.
- Secret runtime hanya berasal dari environment variable atau penyimpanan secret yang disetujui.
- Jangan tampilkan nilai secret dalam error atau handoff.
- Jangan menjalankan tindakan destruktif atau mengakses path di luar workspace tanpa persetujuan eksplisit.

## Verifikasi dan Handoff

- Jalankan command `Verify` task dan pemeriksaan relevan dari `docs/development.md`.
- Catat command serta hasilnya; jangan mengklaim command yang tidak dijalankan.
- Serahkan task memakai format handoff di `docs/development.md`.
- Laporkan `BLOCKED` bila dependency belum selesai dan `NEEDS_REVIEW` bila scope atau keputusan lintas-agent berkonflik.
