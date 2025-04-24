.ORIG   x3000
TRAP    x20
TRAP    x21

AND     R0,R0,#0      ; R0 == 0
ADD     R0,R0,#10     ; R0 == 10
TRAP    x21
HALT

.END
