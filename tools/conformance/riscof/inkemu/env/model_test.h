#ifndef _INKEMU_MODEL_TEST_H
#define _INKEMU_MODEL_TEST_H

#define RVMODEL_DATA_SECTION \
        .pushsection .tohost,"aw",@progbits;                \
        .align 8; .global tohost; tohost: .dword 0;         \
        .align 8; .global fromhost; fromhost: .dword 0;     \
        .popsection;                                        \
        .align 8; .global begin_regstate; begin_regstate:   \
        .word 128;                                          \
        .align 8; .global end_regstate; end_regstate:       \
        .word 4;

// Writing 1 to tohost ends the test (`inkemu rvtest` reports it as "Passed";
// the verdict that matters is the signature comparison done by RISCOF).
#define RVMODEL_HALT                                        \
  li x1, 1;                                                 \
  1:                                                        \
    sw x1, tohost, t5;                                      \
    j 1b;

#define RVMODEL_BOOT
#define RVMODEL_DATA_BEGIN RVMODEL_DATA_SECTION .align 4; .global begin_signature; begin_signature:
// No padding before end_signature: the reference model's env does not pad either,
// and both signatures must have the same length.
#define RVMODEL_DATA_END .global end_signature; end_signature:

#define RVMODEL_IO_INIT
#define RVMODEL_IO_WRITE_STR(_R, _STR)
#define RVMODEL_IO_CHECK()
#define RVMODEL_IO_ASSERT_GPR_EQ(_S, _R, _I)
#define RVMODEL_IO_ASSERT_SFPR_EQ(_F, _R, _I)
#define RVMODEL_IO_ASSERT_DFPR_EQ(_D, _R, _I)

#define RVMODEL_SET_MSW_INT
#define RVMODEL_CLEAR_MSW_INT
#define RVMODEL_CLEAR_MTIMER_INT
#define RVMODEL_CLEAR_MEXT_INT

#endif
