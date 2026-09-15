#include <stdio.h>
#include <stdlib.h>

/* Global variable */
int global_counter = 0;
static int static_var = 42;

/* Simple function */
int add(int a, int b) {
    return a + b;
}

/* Function that calls another function */
void print_sum(int x, int y) {
    int sum = add(x, y);
    printf("Sum: %d\n", sum);
}

/* Function with local variables */
int compute(int base, int factor) {
    int result = base * factor;
    int temp = result + global_counter;
    return temp;
}

int main(int argc, char **argv) {
    int a = 10;
    int b = 20;

    print_sum(a, b);
    global_counter = compute(a, b);

    return 0;
}
