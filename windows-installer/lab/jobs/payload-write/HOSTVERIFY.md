# Verifying the payload-write run from the host

`run.ps1` proves things from inside the guest. That is half the measurement:
the guest is also the thing under test, so its readback is not independent.
`hostverify.sh` is the other half — it reads the qcow2 overlay the guest wrote
through, converts it to a sparse raw, and compares it byte-for-byte against the
pristine backing fixture using tools that have never been near Windows.

Run it after `winlab run payload-write`, before any other `winlab run` (a
later run recreates `run-fixture-a.qcow2`; copy it aside first):

    cp /var/lab-scratch/winlab/run-fixture-a.qcow2 \
       /var/lab-scratch/winlab/payload-write-fixture-a.qcow2

    mkdir -p /var/lab-scratch/<your-slug>
    cp hostverify.sh hostverify.py /var/lab-scratch/<your-slug>/
    podman run --rm \
      -v /var/lab-scratch/winlab:/w:z \
      -v /var/lab-scratch/<your-slug>:/o:z \
      localhost/apex-winlab:latest bash /o/hostverify.sh

It asserts, and fails loudly on any of:

1. the pristine `fixture-a.raw` is still the size the fixture builder made it
   (its mtime is printed, and must be the fixtures-rebuild time — if a guest
   run ever wrote the backing file directly, the whole comparison is void);
2. the payload hashes to the value the guest generated, at the verified offset;
3. partition 1 has exactly one written extent, and it is the payload's;
4. the primary and backup GPT are byte-identical to pristine;
5. no cluster outside the three partitions was written at all;
6. the refused NTFS write's exact target is byte-identical to pristine;
7. the RAW-volume write that Windows did NOT refuse is bounded to the 512
   bytes it wrote, and its content matches what the guest reported.

The expected hashes are constants at the top of `hostverify.py`, taken from
the guest transcript. If the job is re-run with a different payload seed or a
different fixture layout they must be updated together — a stale constant here
fails the run rather than passing it quietly, which is the intended direction.
