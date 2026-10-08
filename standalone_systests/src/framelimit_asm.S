/*
 * Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
 * SPDX-License-Identifier: BSD-3-Clause-Clear
 */

/*
 * Enter guest mode and overflow the frame limit.  The error handler
 * skips allocframe after the stack-limit exception.  Reading SSR then raises
 * the existing privilege exception, whose handler returns to monitor mode.
 */
	.text
	.p2align 4
	.global guest_framelimit_overflow
	.type guest_framelimit_overflow, @function
guest_framelimit_overflow:
	r0 = ##.Lguest
	elr = r0
	r0 = ssr
	r0 = setbit(r0, #17)  /* EX = 1, required for rte */
	r0 = setbit(r0, #16)  /* UM = 1, required for guest mode */
	r0 = setbit(r0, #19)  /* GM = 1 */
	ssr = r0
	isync
	rte

.Lguest:
	allocframe(#4104)
	r0 = ssr              /* Fault and return to monitor mode */
	jumpr r31
	.size guest_framelimit_overflow, . - guest_framelimit_overflow
